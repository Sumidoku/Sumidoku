use serde::Serialize;
use std::collections::HashMap;

pub type Rid = i32;

pub enum StoreItem {
	Encoded(Vec<u8>),
	String(String),
	Request(Box<crate::net::NetRequest>),
}

impl StoreItem {
	pub fn as_encoded(&self) -> Option<&[u8]> {
		match self {
			StoreItem::Encoded(data) => Some(data),
			StoreItem::String(s) => Some(s.as_bytes()),
			_ => None,
		}
	}

	pub fn as_request(&mut self) -> Option<&mut crate::net::NetRequest> {
		if let StoreItem::Request(r) = self { Some(r) } else { None }
	}
}

/// 与生产 `GlobalStore` 相同的 rid 分配语义（1 起递增、清空后归位），
/// 让两引擎跑在同一条 ABI 约定上
pub struct GlobalStore {
	pointer: Rid,
	storage: HashMap<Rid, StoreItem>,
}

impl GlobalStore {
	pub fn new() -> Self {
		GlobalStore { pointer: 1, storage: HashMap::new() }
	}

	pub fn store(&mut self, item: StoreItem) -> Rid {
		let rid = self.pointer;
		self.storage.insert(rid, item);
		self.pointer += 1;
		rid
	}

	pub fn store_encoded<T: Serialize>(&mut self, item: &T) -> Result<Rid, postcard::Error> {
		let encoded = postcard::to_allocvec(item)?;
		Ok(self.store(StoreItem::Encoded(encoded)))
	}

	pub fn get(&self, rid: Rid) -> Option<&StoreItem> {
		self.storage.get(&rid)
	}

	pub fn get_mut(&mut self, rid: Rid) -> Option<&mut StoreItem> {
		self.storage.get_mut(&rid)
	}

	pub fn remove(&mut self, rid: Rid) {
		self.storage.remove(&rid);
		if self.storage.is_empty() {
			self.pointer = 1;
		}
	}

	pub fn take(&mut self, rid: Rid) -> Option<StoreItem> {
		self.storage.remove(&rid)
	}

	pub fn restore(&mut self, rid: Rid, item: StoreItem) {
		self.storage.insert(rid, item);
	}
}

impl Default for GlobalStore {
	fn default() -> Self {
		GlobalStore::new()
	}
}
