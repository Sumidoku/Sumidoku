use crate::{
	WasmEnv,
	html_dom::{HtmlDocument, HtmlNode, QueryRoot, select_str},
	runtime::store::{FFIResult, Ptr, Rid, StoreItem},
};
use wasmer::FunctionEnvMut;

enum Result {
	// Success,
	InvalidDescriptor,
	InvalidString,
	// InvalidHtml,
	InvalidQuery,
	#[allow(clippy::enum_variant_names)]
	NoResult,
	// SwiftSoupError,
}

impl From<Result> for i32 {
	fn from(result: Result) -> Self {
		match result {
			// Result::Success => 0,
			Result::InvalidDescriptor => -1,
			Result::InvalidString => -2,
			Result::InvalidQuery => -4,
			Result::NoResult => -5,
			// Result::SwiftSoupError => -6,
		}
	}
}

enum Kind {
	Unknown,
	Node,
	TextNode,
	// DataNode,
	Comment,
	Element,
	ElementList,
	Document,
}

impl From<Kind> for i32 {
	fn from(value: Kind) -> Self {
		match value {
			Kind::Unknown => 0,
			Kind::Node => 1,
			Kind::TextNode => 2,
			// Kind::DataNode => 3,
			Kind::Comment => 4,
			Kind::Element => 5,
			Kind::ElementList => 6,
			Kind::Document => 7,
		}
	}
}

pub fn parse(
	mut env: FunctionEnvMut<WasmEnv>,
	html_ptr: Ptr,
	html_len: u32,
	base_url_ptr: Ptr,
	base_url_len: u32,
) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Ok(base_url_string) = env.data().read_string(&env, base_url_ptr, base_url_len) else {
		return Result::InvalidString.into();
	};
	let document = HtmlDocument::parse(&text, Some(&base_url_string));
	env.data_mut()
		.store
		.store(StoreItem::HtmlDocument(document))
}
pub fn parse_fragment(
	mut env: FunctionEnvMut<WasmEnv>,
	html_ptr: Ptr,
	html_len: u32,
	base_url_ptr: Ptr,
	base_url_len: u32,
) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Ok(base_url_string) = env.data().read_string(&env, base_url_ptr, base_url_len) else {
		return Result::InvalidString.into();
	};
	let document = HtmlDocument::parse_fragment(&text, Some(&base_url_string));
	env.data_mut()
		.store
		.store(StoreItem::HtmlDocument(document))
}
pub fn escape(mut env: FunctionEnvMut<WasmEnv>, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	env.data_mut().store.store(StoreItem::String(escape_str(&text)))
}
pub fn unescape(mut env: FunctionEnvMut<WasmEnv>, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	env.data_mut()
		.store
		.store(StoreItem::String(unescape_str(&text)))
}

pub(crate) fn escape_str(text: &str) -> String {
	let mut escaped = String::with_capacity(text.len());
	for c in text.chars() {
		match c {
			'&' => escaped.push_str("&amp;"),
			'<' => escaped.push_str("&lt;"),
			'>' => escaped.push_str("&gt;"),
			_ => escaped.push(c),
		}
	}
	escaped
}

pub(crate) fn unescape_str(text: &str) -> String {
	let mut unescaped = String::with_capacity(text.len());
	let mut chars = text.chars().peekable();

	while let Some(c) = chars.next() {
		if c == '&' {
			let mut entity = String::from("&");
			while let Some(&next_c) = chars.peek() {
				entity.push(next_c);
				if next_c == ';' || entity.len() > 10 {
					chars.next();
					break;
				}
				chars.next();
			}
			match entity.as_str() {
				"&amp;" => unescaped.push('&'),
				"&lt;" => unescaped.push('<'),
				"&gt;" => unescaped.push('>'),
				_ => unescaped.push_str(&entity),
			}
		} else {
			unescaped.push(c);
		}
	}
	unescaped
}

pub fn kind(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	fn kind_from_node(node: &HtmlNode) -> Kind {
		if node.is_element() {
			Kind::Element
		} else if node.is_comment() {
			Kind::Comment
		} else if node.is_text() {
			Kind::TextNode
		} else if node.is_document() {
			Kind::Document
		} else {
			Kind::Node
		}
	}
	match item {
		StoreItem::HtmlElement(element) => kind_from_node(&element.0),
		StoreItem::HtmlNode(node) => kind_from_node(node),
		StoreItem::HtmlDocument(_) => Kind::Document,
		StoreItem::HtmlElementList(_) => Kind::ElementList,
		_ => Kind::Unknown,
	}
	.into()
}

pub fn child_nodes(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_node() {
		let Some(nodes) = element.child_nodes() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNodeList(nodes))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn has_attr(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, attr_ptr: Ptr, attr_len: u32) -> i32 {
	let Ok(attr) = env.data().read_string(&env, attr_ptr, attr_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		if element.has_attr(&attr) { 1 } else { 0 }
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn set_attr(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	key_ptr: Ptr,
	key_len: u32,
	value_ptr: Ptr,
	value_len: u32,
) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(value) = env.data().read_string(&env, value_ptr, value_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// 客户端 Node/Element 都有 set_attr；文本/注释节点挂不了属性，报 InvalidDescriptor
	let Some(node) = item.as_html_node() else {
		return Result::InvalidDescriptor.into();
	};
	if !node.is_element() {
		return Result::InvalidDescriptor.into();
	}
	if node.set_attr(&key, &value).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn remove_attr(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	attr_ptr: Ptr,
	attr_len: u32,
) -> FFIResult {
	let Ok(attr) = env.data().read_string(&env, attr_ptr, attr_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(node) = item.as_html_node() else {
		return Result::InvalidDescriptor.into();
	};
	if !node.is_element() {
		return Result::InvalidDescriptor.into();
	}
	if node.remove_attr(&attr).is_none() {
		return Result::NoResult.into();
	}
	0
}

pub fn set_text(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = env.data().read_string(&env, text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.set_text(&text).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn set_html(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = env.data().read_string(&env, html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.set_html(&html).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn prepend(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = env.data().read_string(&env, html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.prepend_html(&html).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn append(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = env.data().read_string(&env, html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.append_html(&html).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn children(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(elements) = element.children() else {
			return Result::NoResult.into();
		};
		env.data_mut()
			.store
			.store(StoreItem::HtmlElementList(elements))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn base_uri(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};

	if let Some(element) = item.as_html_element() {
		let Some(base_uri) = element.0.base_uri.as_ref() else {
			return Result::NoResult.into();
		};
		let uri = base_uri.to_string();
		env.data_mut().store.store(StoreItem::String(uri))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn own_text(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(text) = element.own_text() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::String(text))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn data(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(data) = element.data() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::String(data))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn id(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(id) = element.id() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::String(id))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn tag_name(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(tag) = element.tag_name() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::String(tag))
	} else {
		Result::InvalidDescriptor.into()
	}
}

pub fn class_name(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(class) = element.class_name() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::String(class))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn has_class(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> i32 {
	let Ok(class) = env.data().read_string(&env, class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		if element.has_class(&class) { 1 } else { 0 }
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn add_class(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> FFIResult {
	let Ok(class) = env.data().read_string(&env, class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.add_class(&class).is_none() {
		return Result::NoResult.into();
	}
	0
}
pub fn remove_class(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> FFIResult {
	let Ok(class) = env.data().read_string(&env, class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let Some(element) = item.as_html_element() else {
		return Result::InvalidDescriptor.into();
	};
	if element.0.remove_class(&class).is_none() {
		return Result::NoResult.into();
	}
	0
}

pub fn first(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.first().cloned() else {
			return Result::InvalidDescriptor.into();
		};
		env.data_mut().store.store(StoreItem::HtmlElement(element))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn last(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.last().cloned() else {
			return Result::InvalidDescriptor.into();
		};
		env.data_mut().store.store(StoreItem::HtmlElement(element))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn get(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, index: u32) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.get(index as usize).cloned() else {
			return Result::InvalidDescriptor.into();
		};
		env.data_mut().store.store(StoreItem::HtmlElement(element))
	} else if let StoreItem::HtmlNodeList(nodes) = item {
		let Some(node) = nodes.get(index as usize).cloned() else {
			return Result::InvalidDescriptor.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNode(node))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn size(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	match item {
		StoreItem::HtmlNodeList(list) => list.len() as FFIResult,
		StoreItem::HtmlElementList(elements) => elements.0.len() as i32,
		_ => Result::InvalidDescriptor.into(),
	}
}

pub fn parent(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(parent) = element.parent() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlElement(parent))
	} else if let Some(node) = item.as_html_node() {
		let Some(parent) = node.parent() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNode(parent))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn siblings(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(elements) = element.siblings() else {
			return Result::NoResult.into();
		};
		env.data_mut()
			.store
			.store(StoreItem::HtmlElementList(elements))
	} else if let Some(node) = item.as_html_node() {
		let Some(nodes) = node.siblings() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNodeList(nodes))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn next(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(next_element) = element.next_sibling() else {
			return Result::NoResult.into();
		};
		env.data_mut()
			.store
			.store(StoreItem::HtmlElement(next_element))
	} else if let Some(node) = item.as_html_node() {
		let Some(next_node) = node.next_sibling() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNode(next_node))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn previous(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(prev_element) = element.prev_sibling() else {
			return Result::NoResult.into();
		};
		env.data_mut()
			.store
			.store(StoreItem::HtmlElement(prev_element))
	} else if let Some(node) = item.as_html_node() {
		let Some(prev_node) = node.prev_sibling() else {
			return Result::NoResult.into();
		};
		env.data_mut().store.store(StoreItem::HtmlNode(prev_node))
	} else {
		Result::InvalidDescriptor.into()
	}
}

pub fn attr(mut env: FunctionEnvMut<WasmEnv>, rid: Rid, key_ptr: u32, key_len: u32) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let attr = if let Some(element) = item.as_html_element() {
		element.attr(&key)
	} else if let Some(elements) = item.as_html_element_list() {
		elements.attr(&key)
	} else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(attr) = attr {
		env.data_mut().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn outer_html(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let attr = if let Some(element) = item.as_html_element() {
		element.outer_html()
	} else if let Some(elements) = item.as_html_element_list() {
		elements.outer_html()
	} else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(attr) = attr {
		env.data_mut().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn remove(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// 客户端 Element.remove 与 ElementList.remove 共用此函数
	if let Some(element) = item.as_html_element() {
		if element.0.detach().is_none() {
			return Result::NoResult.into();
		}
		return 0;
	}
	if let Some(elements) = item.as_html_element_list() {
		for element in &elements.0 {
			if element.0.detach().is_none() {
				return Result::NoResult.into();
			}
		}
		return 0;
	}
	Result::InvalidDescriptor.into()
}

pub fn select(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	query_ptr: Ptr,
	query_len: u32,
) -> FFIResult {
	let Ok(string) = env.data().read_string(&env, query_ptr, query_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// document 臂存空列表（SwiftSoup select 语义），元素/列表臂空结果报 NoResult
	let (root, store_empty) = if let Some(document) = item.as_html_document() {
		(QueryRoot::Document(document), true)
	} else if let Some(element) = item.as_html_element() {
		(QueryRoot::Element(element), false)
	} else if let Some(elements) = item.as_html_element_list() {
		(QueryRoot::List(elements), false)
	} else {
		return Result::InvalidDescriptor.into();
	};
	match select_str(root, &string) {
		Ok(elements) if store_empty || !elements.0.is_empty() => env
			.data_mut()
			.store
			.store(StoreItem::HtmlElementList(elements)),
		Ok(_) => Result::NoResult.into(),
		Err(()) => Result::InvalidQuery.into(),
	}
}
pub fn select_first(
	mut env: FunctionEnvMut<WasmEnv>,
	rid: Rid,
	query_ptr: Ptr,
	query_len: u32,
) -> FFIResult {
	let Ok(string) = env.data().read_string(&env, query_ptr, query_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let root = if let Some(document) = item.as_html_document() {
		QueryRoot::Document(document)
	} else if let Some(element) = item.as_html_element() {
		QueryRoot::Element(element)
	} else if let Some(elements) = item.as_html_element_list() {
		QueryRoot::List(elements)
	} else {
		return Result::InvalidDescriptor.into();
	};
	match select_str(root, &string) {
		Ok(elements) => match elements.0.into_iter().next() {
			Some(first) => env.data_mut().store.store(StoreItem::HtmlElement(first)),
			None => Result::NoResult.into(),
		},
		Err(()) => Result::InvalidQuery.into(),
	}
}
pub fn text(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let text = if let Some(element) = item.as_html_element() {
		element.text(true)
	} else if let Some(node) = item.as_html_node() {
		node.text()
	} else if let Some(elements) = item.as_html_element_list() {
		elements.text(true)
	} else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(text) = text {
		env.data_mut().store.store(StoreItem::String(text))
	} else {
		Result::NoResult.into()
	}
}
pub fn untrimmed_text(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let attr = if let Some(element) = item.as_html_element() {
		element.text(false)
	} else if let Some(elements) = item.as_html_element_list() {
		elements.text(false)
	} else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(attr) = attr {
		env.data_mut().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn html(mut env: FunctionEnvMut<WasmEnv>, rid: Rid) -> FFIResult {
	let Some(item) = env.data_mut().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	let attr = if let Some(element) = item.as_html_element() {
		element.html()
	} else if let Some(elements) = item.as_html_element_list() {
		elements.html()
	} else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(attr) = attr {
		env.data_mut().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		runtime::{
			env::{Defaults, NetState},
			limits::Limits,
			store::GlobalStore,
		},
		traits::{DirectPolicy, NoopDefaultsSink, StdoutLogger},
	};
	use std::{collections::HashMap, sync::Arc};
	use wasmer::{Function, FunctionEnv, Memory, MemoryType, Pages, Store, Value};

	struct Harness {
		store: Store,
		env: FunctionEnv<WasmEnv>,
		memory: Memory,
		bump: u32,
	}

	impl Harness {
		fn new() -> Self {
			let mut store = Store::default();
			let memory = Memory::new(&mut store, MemoryType::new(Pages(4), None, false)).unwrap();
			let env = FunctionEnv::new(
				&mut store,
				WasmEnv {
					memory: Some(memory.clone()),
					store: GlobalStore::new(),
					defaults: Defaults::new(HashMap::new(), Arc::new(NoopDefaultsSink), "test".into()),
					net: NetState::new(Arc::new(DirectPolicy { user_agent: None })),
					limits: Limits::default(),
					source_key: "test".into(),
					logger: Arc::new(StdoutLogger),
				},
			);
			// 指针 0 起是合法内存，测试数据从 1024 起 bump 写入避免踩到空指针路径
			Self { store, env, memory, bump: 1024 }
		}

		fn write_bytes(&mut self, bytes: &[u8]) -> (i32, i32) {
			let ptr = self.bump;
			self.bump += bytes.len() as u32 + 64;
			self.memory.view(&self.store).write(ptr as u64, bytes).unwrap();
			(ptr as i32, bytes.len() as i32)
		}

		fn write_str(&mut self, s: &str) -> (i32, i32) {
			self.write_bytes(s.as_bytes())
		}

		fn stored_string(&self, rid: Rid) -> String {
			let Some(StoreItem::String(s)) = self.env.as_ref(&self.store).store.get(rid) else {
				panic!("rid {rid} 不是 StoreItem::String");
			};
			s.clone()
		}

		fn parse_doc(&mut self, html: &str) -> Rid {
			let (p, l) = self.write_str(html);
			self.call4(parse, p, l, 0, 0)
		}

		fn select_first(&mut self, rid: Rid, query: &str) -> Rid {
			let (p, l) = self.write_str(query);
			self.call3(select_first, rid, p, l)
		}

		fn select(&mut self, rid: Rid, query: &str) -> Rid {
			let (p, l) = self.write_str(query);
			self.call3(select, rid, p, l)
		}

		fn text_of(&mut self, rid: Rid) -> String {
			let string_rid = self.call1(text, rid);
			assert!(string_rid > 0, "text 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn html_of(&mut self, rid: Rid) -> String {
			let string_rid = self.call1(html, rid);
			assert!(string_rid > 0, "html 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn outer_html_of(&mut self, rid: Rid) -> String {
			let string_rid = self.call1(outer_html, rid);
			assert!(string_rid > 0, "outer_html 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn attr_of(&mut self, rid: Rid, key: &str) -> Rid {
			let (p, l) = self.write_str(key);
			self.call3(attr, rid, p, l)
		}

		fn class_name_of(&mut self, rid: Rid) -> String {
			let string_rid = self.call1(class_name, rid);
			assert!(string_rid > 0, "class_name 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn call1(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Rid) -> FFIResult, a: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		fn call1b(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Rid, Ptr, u32) -> i32, a: i32, b: i32, c: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a), Value::I32(b), Value::I32(c)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		fn call3(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Rid, Ptr, u32) -> FFIResult, a: i32, b: i32, c: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a), Value::I32(b), Value::I32(c)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		fn call4(&mut self, f: fn(FunctionEnvMut<WasmEnv>, Ptr, u32, Ptr, u32) -> FFIResult, a: i32, b: i32, c: i32, d: i32) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(&mut self.store, &[Value::I32(a), Value::I32(b), Value::I32(c), Value::I32(d)])
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}

		fn call5(
			&mut self,
			f: fn(FunctionEnvMut<WasmEnv>, Rid, Ptr, u32, Ptr, u32) -> FFIResult,
			a: i32,
			b: i32,
			c: i32,
			d: i32,
			e: i32,
		) -> i32 {
			Function::new_typed_with_env(&mut self.store, &self.env, f)
				.call(
					&mut self.store,
					&[Value::I32(a), Value::I32(b), Value::I32(c), Value::I32(d), Value::I32(e)],
				)
				.unwrap()
				.first()
				.unwrap()
				.i32()
				.unwrap()
		}
	}

	const DOC: &str = r#"<!DOCTYPE html><html><body>
<div id="a" class="x y"><p class="del">gone</p><p>keep</p></div>
</body></html>"#;

	// 无效的 UTF-8（0xFF）触发 read_string 的 Utf8 错误，host 侧统一报 InvalidString
	fn invalid_utf8(h: &mut Harness) -> (i32, i32) {
		h.write_bytes(&[0xFF, 0xFE])
	}

	#[test]
	fn set_attr_and_remove_attr() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		assert!(el > 0);

		let (kp, kl) = h.write_str("href");
		let (vp, vl) = h.write_str("/x");
		assert_eq!(h.call5(set_attr, el, kp, kl, vp, vl), 0);
		let attr_rid = h.attr_of(el, "href");
		assert_eq!(h.stored_string(attr_rid), "/x");

		// 已存在的 key 覆盖旧值
		let (vp, vl) = h.write_str("/y");
		assert_eq!(h.call5(set_attr, el, kp, kl, vp, vl), 0);
		let attr_rid = h.attr_of(el, "href");
		assert_eq!(h.stored_string(attr_rid), "/y");

		assert_eq!(h.call3(remove_attr, el, kp, kl), 0);
		assert_eq!(h.attr_of(el, "href"), -5);
		// jsoup removeAttr 对不存在的属性也是成功
		assert_eq!(h.call3(remove_attr, el, kp, kl), 0);
	}

	#[test]
	fn set_attr_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (kp, kl) = h.write_str("href");
		let (vp, vl) = h.write_str("/x");

		// 正例锚点：错误码断言必须建立在函数已实现的前提下（stub 全返回 -1 会造成假绿）
		assert_eq!(h.call5(set_attr, el, kp, kl, vp, vl), 0);
		// 无效 descriptor
		assert_eq!(h.call5(set_attr, 9999, kp, kl, vp, vl), -1);
		assert_eq!(h.call3(remove_attr, 9999, kp, kl), -1);
		// document 不是元素，attr 操作报 InvalidDescriptor
		assert_eq!(h.call5(set_attr, doc, kp, kl, vp, vl), -1);
		assert_eq!(h.call3(remove_attr, doc, kp, kl), -1);
		// 无效字符串（非法 UTF-8）
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(h.call5(set_attr, el, bp, bl, vp, vl), -2);
		assert_eq!(h.call5(set_attr, el, kp, kl, bp, bl), -2);
		assert_eq!(h.call3(remove_attr, el, bp, bl), -2);
	}

	#[test]
	fn set_text_replaces_children() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let p = h.select_first(doc, "p.del");
		assert!(p > 0);

		let (tp, tl) = h.write_str("new & <b>");
		assert_eq!(h.call3(set_text, p, tp, tl), 0);
		assert_eq!(h.text_of(p), "new & <b>");
		assert_eq!(h.outer_html_of(p), r#"<p class="del">new &amp; &lt;b&gt;</p>"#);
		// 同一个文档上的其他查询能看到 mutation
		let again = h.select_first(doc, "p.del");
		assert_eq!(h.text_of(again), "new & <b>");
	}

	#[test]
	fn set_text_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (tp, tl) = h.write_str("x");

		// 正例锚点，防 stub 假绿
		assert_eq!(h.call3(set_text, el, tp, tl), 0);
		assert_eq!(h.call3(set_text, 9999, tp, tl), -1);
		assert_eq!(h.call3(set_text, doc, tp, tl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(h.call3(set_text, el, bp, bl), -2);
	}

	#[test]
	fn set_html_replaces_inner() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");

		let (hp, hl) = h.write_str(r#"<span class="s">hi</span> tail"#);
		assert_eq!(h.call3(set_html, el, hp, hl), 0);
		assert_eq!(h.html_of(el), r#"<span class="s">hi</span> tail"#);
		// 解析出的 fragment 是真实节点，可以继续 select
		let span = h.select_first(el, "span.s");
		assert!(span > 0);
		assert_eq!(h.text_of(span), "hi");
		// 旧子节点已被替换
		assert_eq!(h.select_first(el, "p.del"), -5);
	}

	#[test]
	fn set_html_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (hp, hl) = h.write_str("<b>x</b>");

		// 正例锚点，防 stub 假绿
		assert_eq!(h.call3(set_html, el, hp, hl), 0);
		assert_eq!(h.call3(set_html, 9999, hp, hl), -1);
		assert_eq!(h.call3(set_html, doc, hp, hl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(h.call3(set_html, el, bp, bl), -2);
	}

	#[test]
	fn prepend_and_append_splice_fragments() {
		let mut h = Harness::new();
		let doc = h.parse_doc(r#"<!DOCTYPE html><html><body><div id="a"><p>mid</p></div></body></html>"#);
		let el = h.select_first(doc, "#a");

		let (p1, l1) = h.write_str("<b>first</b>");
		assert_eq!(h.call3(prepend, el, p1, l1), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p>");

		let (p2, l2) = h.write_str("<i>last</i>");
		assert_eq!(h.call3(append, el, p2, l2), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p><i>last</i>");

		// 多节点 fragment 保持相对顺序
		let (p3, l3) = h.write_str("t1<s>1</s>t2");
		assert_eq!(h.call3(append, el, p3, l3), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p><i>last</i>t1<s>1</s>t2");

		let (p4, l4) = h.write_str("h1<u>2</u>");
		assert_eq!(h.call3(prepend, el, p4, l4), 0);
		assert_eq!(h.html_of(el), "h1<u>2</u><b>first</b><p>mid</p><i>last</i>t1<s>1</s>t2");
	}

	#[test]
	fn prepend_append_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (hp, hl) = h.write_str("<b>x</b>");

		// 正例锚点，防 stub 假绿
		assert_eq!(h.call3(prepend, el, hp, hl), 0);
		assert_eq!(h.call3(append, el, hp, hl), 0);
		assert_eq!(h.call3(prepend, 9999, hp, hl), -1);
		assert_eq!(h.call3(append, 9999, hp, hl), -1);
		assert_eq!(h.call3(prepend, doc, hp, hl), -1);
		assert_eq!(h.call3(append, doc, hp, hl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(h.call3(prepend, el, bp, bl), -2);
		assert_eq!(h.call3(append, el, bp, bl), -2);
	}

	#[test]
	fn add_class_and_remove_class() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		assert_eq!(h.class_name_of(el), "x y");

		let (cp, cl) = h.write_str("z");
		assert_eq!(h.call3(add_class, el, cp, cl), 0);
		assert_eq!(h.class_name_of(el), "x y z");
		assert_eq!(h.call1b(has_class, el, cp, cl), 1);
		// 重复添加是幂等的（jsoup 集合语义）
		assert_eq!(h.call3(add_class, el, cp, cl), 0);
		assert_eq!(h.class_name_of(el), "x y z");
		// 新 class 对后续 select 可见
		assert!(h.select_first(doc, "#a.z") > 0);

		let (yp, yl) = h.write_str("y");
		assert_eq!(h.call3(remove_class, el, yp, yl), 0);
		assert_eq!(h.class_name_of(el), "x z");
		// 移除不存在的 class 也是成功
		assert_eq!(h.call3(remove_class, el, yp, yl), 0);
		assert_eq!(h.class_name_of(el), "x z");
		assert!(h.select_first(doc, "#a.y") == -5);
	}

	#[test]
	fn class_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (cp, cl) = h.write_str("z");

		// 正例锚点，防 stub 假绿
		assert_eq!(h.call3(add_class, el, cp, cl), 0);
		assert_eq!(h.call3(remove_class, el, cp, cl), 0);
		assert_eq!(h.call3(add_class, 9999, cp, cl), -1);
		assert_eq!(h.call3(remove_class, 9999, cp, cl), -1);
		assert_eq!(h.call3(add_class, doc, cp, cl), -1);
		assert_eq!(h.call3(remove_class, doc, cp, cl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(h.call3(add_class, el, bp, bl), -2);
		assert_eq!(h.call3(remove_class, el, bp, bl), -2);
	}

	#[test]
	fn remove_detaches_element() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let p = h.select_first(doc, "p.del");
		assert!(p > 0);

		assert_eq!(h.call1(remove, p), 0);
		// 文档里查不到了
		let list = h.select(doc, "p");
		assert!(list > 0);
		assert_eq!(h.call1(size, list), 1);
		let remaining = h.select_first(doc, "p");
		assert_eq!(h.text_of(remaining), "keep");
		// 被摘下的元素自身仍可读写（ego-tree detach 保留子树，对齐 jsoup remove 后元素仍可用）
		assert_eq!(h.text_of(p), "gone");
		assert_eq!(h.call1(parent, p), -5);
	}

	#[test]
	fn remove_element_list() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let list = h.select(doc, "p");
		assert!(list > 0);
		assert_eq!(h.call1(size, list), 2);

		assert_eq!(h.call1(remove, list), 0);
		let after = h.select(doc, "p");
		assert!(after > 0);
		assert_eq!(h.call1(size, after), 0);
	}

	#[test]
	fn remove_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");

		// 正例锚点，防 stub 假绿
		assert_eq!(h.call1(remove, el), 0);
		assert_eq!(h.call1(remove, 9999), -1);
		// document 不是 Element/ElementList
		assert_eq!(h.call1(remove, doc), -1);
	}
}
