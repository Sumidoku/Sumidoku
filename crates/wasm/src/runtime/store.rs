use crate::html_dom::{HtmlDocument, HtmlElement, HtmlElementList, HtmlNode};
use crate::js_engine::JsEngine;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type Rid = i32;
pub type FFIResult = i32;
pub type Ptr = u32;

/// 统一 RGBA8 直色（每像素 4 字节）：canvas 绘制与 net::get_image 共用同一
/// 像素格式，get_image_data 才能无差别编码。
/// premultiply/demultiply 只在进出 tiny-skia 时发生
pub struct ImageData {
	pub data: Vec<u8>,
	pub width: i32,
	pub height: i32,
}

impl ImageData {
	pub fn from_rgba8(data: Vec<u8>, width: u32, height: u32) -> Self {
		Self {
			data,
			width: width as i32,
			height: height as i32,
		}
	}

	pub fn encode_png(&self) -> Option<Vec<u8>> {
		use image::ImageEncoder;
		let mut png = Vec::new();
		let encoder = image::codecs::png::PngEncoder::new(&mut png);
		encoder
			.write_image(
				&self.data,
				self.width as u32,
				self.height as u32,
				image::ExtendedColorType::Rgba8,
			)
			.ok()?;
		Some(png)
	}
}

pub struct CanvasState {
	pub pixmap: tiny_skia::Pixmap,
	pub transform: tiny_skia::Transform,
}

pub enum StoreItem {
	String(String),
	Request(Box<crate::imports::NetRequest>),
	HtmlDocument(HtmlDocument),
	HtmlElement(HtmlElement),
	HtmlNode(HtmlNode),
	HtmlNodeList(Vec<HtmlNode>),
	HtmlElementList(HtmlElementList),
	Encoded(Vec<u8>),
	/// 处理器入参里宿主解不出的原始响应字节（加密图床，见 process_image 的
	/// 解码回退）：没解码就没有像素——draw/copy 不认它，width/height 落
	/// InvalidImagePointer，get_image_data 与透传取图把字节原样交还
	ImageBytes(Vec<u8>),
	ImageData(ImageData),
	Canvas(Box<CanvasState>),
	Font(fontdb::ID),
	JsContext(Box<JsEngine>),
	WebView(Box<crate::webview::WebView>),
}

impl StoreItem {
	pub fn as_string(&self) -> Option<&String> {
		if let StoreItem::String(s) = self { Some(s) } else { None }
	}

	pub fn as_request(&mut self) -> Option<&mut crate::imports::NetRequest> {
		if let StoreItem::Request(r) = self { Some(r) } else { None }
	}

	pub fn as_html_document(&mut self) -> Option<&mut HtmlDocument> {
		if let StoreItem::HtmlDocument(h) = self { Some(h) } else { None }
	}

	pub fn as_html_node(&mut self) -> Option<&mut HtmlNode> {
		if let StoreItem::HtmlNode(h) = self {
			Some(h)
		} else if let StoreItem::HtmlElement(h) = self {
			Some(&mut h.0)
		} else {
			None
		}
	}

	pub fn as_html_element(&mut self) -> Option<&mut HtmlElement> {
		if let StoreItem::HtmlElement(h) = self { Some(h) } else { None }
	}

	pub fn as_html_element_list(&mut self) -> Option<&mut HtmlElementList> {
		if let StoreItem::HtmlElementList(h) = self { Some(h) } else { None }
	}

	pub fn as_image_data(&self) -> Option<&ImageData> {
		if let StoreItem::ImageData(s) = self { Some(s) } else { None }
	}

	pub fn as_canvas(&mut self) -> Option<&mut CanvasState> {
		if let StoreItem::Canvas(c) = self { Some(c) } else { None }
	}

	pub fn as_font(&self) -> Option<fontdb::ID> {
		if let StoreItem::Font(id) = self { Some(*id) } else { None }
	}

	pub fn as_js_context(&mut self) -> Option<&mut JsEngine> {
		if let StoreItem::JsContext(engine) = self { Some(engine) } else { None }
	}

	pub fn as_webview(&mut self) -> Option<&mut crate::webview::WebView> {
		if let StoreItem::WebView(view) = self { Some(view) } else { None }
	}

	pub fn as_encoded(&self) -> Option<&[u8]> {
		if let StoreItem::Encoded(data) = self {
			Some(data)
		} else if let StoreItem::String(s) = self {
			Some(s.as_bytes())
		} else {
			None
		}
	}
}

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

	/// 临时取出条目：draw_image 等同时借用 canvas 与 image 的场合，
	/// 取出后必须 restore 放回原 rid
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DefaultValue {
	Data(Vec<u8>),
	Bool(bool),
	Int(i32),
	Float(f32),
	String(String),
	StringArray(Vec<String>),
	Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultKind {
	Data,
	Bool,
	Int,
	Float,
	String,
	StringArray,
	Null,
}

impl From<u8> for DefaultKind {
	fn from(value: u8) -> Self {
		match value {
			0 => Self::Data,
			1 => Self::Bool,
			2 => Self::Int,
			3 => Self::Float,
			4 => Self::String,
			5 => Self::StringArray,
			6 => Self::Null,
			_ => Self::Null,
		}
	}
}
