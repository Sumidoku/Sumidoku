use crate::{
	html_dom::{HtmlDocument, HtmlNode, QueryRoot, select_str},
	runtime::{
		env::Ctx,
		store::{FFIResult, Ptr, Rid, StoreItem},
	},
};

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
	ctx: &mut impl Ctx,
	html_ptr: Ptr,
	html_len: u32,
	base_url_ptr: Ptr,
	base_url_len: u32,
) -> FFIResult {
	let Ok(text) = ctx.read_string(html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Ok(base_url_string) = ctx.read_string(base_url_ptr, base_url_len) else {
		return Result::InvalidString.into();
	};
	let document = HtmlDocument::parse(&text, Some(&base_url_string));
	ctx.host()
		.store
		.store(StoreItem::HtmlDocument(document))
}
pub fn parse_fragment(
	ctx: &mut impl Ctx,
	html_ptr: Ptr,
	html_len: u32,
	base_url_ptr: Ptr,
	base_url_len: u32,
) -> FFIResult {
	let Ok(text) = ctx.read_string(html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Ok(base_url_string) = ctx.read_string(base_url_ptr, base_url_len) else {
		return Result::InvalidString.into();
	};
	let document = HtmlDocument::parse_fragment(&text, Some(&base_url_string));
	ctx.host()
		.store
		.store(StoreItem::HtmlDocument(document))
}
pub fn escape(ctx: &mut impl Ctx, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = ctx.read_string(text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	ctx.host().store.store(StoreItem::String(escape_str(&text)))
}
pub fn unescape(ctx: &mut impl Ctx, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = ctx.read_string(text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	ctx.host()
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

pub fn kind(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
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

pub fn child_nodes(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_node() {
		let Some(nodes) = element.child_nodes() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlNodeList(nodes))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn has_attr(ctx: &mut impl Ctx, rid: Rid, attr_ptr: Ptr, attr_len: u32) -> i32 {
	let Ok(attr) = ctx.read_string(attr_ptr, attr_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		if element.has_attr(&attr) { 1 } else { 0 }
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn set_attr(
	ctx: &mut impl Ctx,
	rid: Rid,
	key_ptr: Ptr,
	key_len: u32,
	value_ptr: Ptr,
	value_len: u32,
) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Ok(value) = ctx.read_string(value_ptr, value_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// The client has set_attr on both Node and Element; text/comment nodes cannot hold attributes, so report InvalidDescriptor
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
	ctx: &mut impl Ctx,
	rid: Rid,
	attr_ptr: Ptr,
	attr_len: u32,
) -> FFIResult {
	let Ok(attr) = ctx.read_string(attr_ptr, attr_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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

pub fn set_text(ctx: &mut impl Ctx, rid: Rid, text_ptr: Ptr, text_len: u32) -> FFIResult {
	let Ok(text) = ctx.read_string(text_ptr, text_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
pub fn set_html(ctx: &mut impl Ctx, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = ctx.read_string(html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
pub fn prepend(ctx: &mut impl Ctx, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = ctx.read_string(html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
pub fn append(ctx: &mut impl Ctx, rid: Rid, html_ptr: Ptr, html_len: u32) -> FFIResult {
	let Ok(html) = ctx.read_string(html_ptr, html_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
pub fn children(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(elements) = element.children() else {
			return Result::NoResult.into();
		};
		ctx.host()
			.store
			.store(StoreItem::HtmlElementList(elements))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn base_uri(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};

	if let Some(element) = item.as_html_element() {
		let Some(base_uri) = element.0.base_uri.as_ref() else {
			return Result::NoResult.into();
		};
		let uri = base_uri.to_string();
		ctx.host().store.store(StoreItem::String(uri))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn own_text(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(text) = element.own_text() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::String(text))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn data(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(data) = element.data() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::String(data))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn id(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(id) = element.id() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::String(id))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn tag_name(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(tag) = element.tag_name() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::String(tag))
	} else {
		Result::InvalidDescriptor.into()
	}
}

pub fn class_name(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(class) = element.class_name() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::String(class))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn has_class(
	ctx: &mut impl Ctx,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> i32 {
	let Ok(class) = ctx.read_string(class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		if element.has_class(&class) { 1 } else { 0 }
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn add_class(
	ctx: &mut impl Ctx,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> FFIResult {
	let Ok(class) = ctx.read_string(class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
	ctx: &mut impl Ctx,
	rid: Rid,
	class_ptr: Ptr,
	class_len: u32,
) -> FFIResult {
	let Ok(class) = ctx.read_string(class_ptr, class_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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

pub fn first(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.first().cloned() else {
			return Result::InvalidDescriptor.into();
		};
		ctx.host().store.store(StoreItem::HtmlElement(element))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn last(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.last().cloned() else {
			return Result::InvalidDescriptor.into();
		};
		ctx.host().store.store(StoreItem::HtmlElement(element))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn get(ctx: &mut impl Ctx, rid: Rid, index: u32) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(elements) = item.as_html_element_list() {
		let Some(element) = elements.0.get(index as usize).cloned() else {
			return Result::InvalidDescriptor.into();
		};
		ctx.host().store.store(StoreItem::HtmlElement(element))
	} else if let StoreItem::HtmlNodeList(nodes) = item {
		let Some(node) = nodes.get(index as usize).cloned() else {
			return Result::InvalidDescriptor.into();
		};
		ctx.host().store.store(StoreItem::HtmlNode(node))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn size(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	match item {
		StoreItem::HtmlNodeList(list) => list.len() as FFIResult,
		StoreItem::HtmlElementList(elements) => elements.0.len() as i32,
		_ => Result::InvalidDescriptor.into(),
	}
}

pub fn parent(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(parent) = element.parent() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlElement(parent))
	} else if let Some(node) = item.as_html_node() {
		let Some(parent) = node.parent() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlNode(parent))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn siblings(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(elements) = element.siblings() else {
			return Result::NoResult.into();
		};
		ctx.host()
			.store
			.store(StoreItem::HtmlElementList(elements))
	} else if let Some(node) = item.as_html_node() {
		let Some(nodes) = node.siblings() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlNodeList(nodes))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn next(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(next_element) = element.next_sibling() else {
			return Result::NoResult.into();
		};
		ctx.host()
			.store
			.store(StoreItem::HtmlElement(next_element))
	} else if let Some(node) = item.as_html_node() {
		let Some(next_node) = node.next_sibling() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlNode(next_node))
	} else {
		Result::InvalidDescriptor.into()
	}
}
pub fn previous(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	if let Some(element) = item.as_html_element() {
		let Some(prev_element) = element.prev_sibling() else {
			return Result::NoResult.into();
		};
		ctx.host()
			.store
			.store(StoreItem::HtmlElement(prev_element))
	} else if let Some(node) = item.as_html_node() {
		let Some(prev_node) = node.prev_sibling() else {
			return Result::NoResult.into();
		};
		ctx.host().store.store(StoreItem::HtmlNode(prev_node))
	} else {
		Result::InvalidDescriptor.into()
	}
}

pub fn attr(ctx: &mut impl Ctx, rid: Rid, key_ptr: u32, key_len: u32) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, key_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
		ctx.host().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn outer_html(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
		ctx.host().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn remove(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// The client's Element.remove and ElementList.remove share this function
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
	ctx: &mut impl Ctx,
	rid: Rid,
	query_ptr: Ptr,
	query_len: u32,
) -> FFIResult {
	let Ok(string) = ctx.read_string(query_ptr, query_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
		return Result::InvalidDescriptor.into();
	};
	// The Aidoku host's select does not distinguish Document/Element, and SwiftSoup
	// returns empty Elements on no match — an empty result always stores an empty list;
	// None is reserved for invalid queries
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
		Ok(elements) => ctx.host()
			.store
			.store(StoreItem::HtmlElementList(elements)),
		Err(()) => Result::InvalidQuery.into(),
	}
}
pub fn select_first(
	ctx: &mut impl Ctx,
	rid: Rid,
	query_ptr: Ptr,
	query_len: u32,
) -> FFIResult {
	let Ok(string) = ctx.read_string(query_ptr, query_len) else {
		return Result::InvalidString.into();
	};
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
			Some(first) => ctx.host().store.store(StoreItem::HtmlElement(first)),
			None => Result::NoResult.into(),
		},
		Err(()) => Result::InvalidQuery.into(),
	}
}
pub fn text(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
		ctx.host().store.store(StoreItem::String(text))
	} else {
		Result::NoResult.into()
	}
}
pub fn untrimmed_text(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
		ctx.host().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}
pub fn html(ctx: &mut impl Ctx, rid: Rid) -> FFIResult {
	let Some(item) = ctx.host().store.get_mut(rid) else {
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
		ctx.host().store.store(StoreItem::String(attr))
	} else {
		Result::NoResult.into()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::WasmEnv;
	use crate::runtime::{env::test_ctx::TestCtx, limits::Limits};

	struct Harness {
		ctx: TestCtx,
	}

	impl Harness {
		fn new() -> Self {
			let env = WasmEnv::for_test(
				Limits::default(),
				crate::browser::BrowserState::unconfigured(std::time::Duration::from_secs(1)),
			);
			Self { ctx: TestCtx::new(env) }
		}

		fn write_bytes(&mut self, bytes: &[u8]) -> (u32, u32) {
			self.ctx.write_bytes(bytes)
		}

		fn write_str(&mut self, s: &str) -> (u32, u32) {
			self.ctx.write_str(s)
		}

		fn stored_string(&self, rid: Rid) -> String {
			let Some(StoreItem::String(s)) = self.ctx.env.store.get(rid) else {
				panic!("rid {rid} 不是 StoreItem::String");
			};
			s.clone()
		}

		fn parse_doc(&mut self, html: &str) -> Rid {
			let (p, l) = self.write_str(html);
			parse(&mut self.ctx, p, l, 0, 0)
		}

		fn select_first(&mut self, rid: Rid, query: &str) -> Rid {
			let (p, l) = self.write_str(query);
			select_first(&mut self.ctx, rid, p, l)
		}

		fn select(&mut self, rid: Rid, query: &str) -> Rid {
			let (p, l) = self.write_str(query);
			select(&mut self.ctx, rid, p, l)
		}

		fn text_of(&mut self, rid: Rid) -> String {
			let string_rid = text(&mut self.ctx, rid);
			assert!(string_rid > 0, "text 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn html_of(&mut self, rid: Rid) -> String {
			let string_rid = html(&mut self.ctx, rid);
			assert!(string_rid > 0, "html 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn outer_html_of(&mut self, rid: Rid) -> String {
			let string_rid = outer_html(&mut self.ctx, rid);
			assert!(string_rid > 0, "outer_html 返回 {string_rid}");
			self.stored_string(string_rid)
		}

		fn attr_of(&mut self, rid: Rid, key: &str) -> Rid {
			let (p, l) = self.write_str(key);
			attr(&mut self.ctx, rid, p, l)
		}

		fn class_name_of(&mut self, rid: Rid) -> String {
			let string_rid = class_name(&mut self.ctx, rid);
			assert!(string_rid > 0, "class_name 返回 {string_rid}");
			self.stored_string(string_rid)
		}
	}

	const DOC: &str = r#"<!DOCTYPE html><html><body>
<div id="a" class="x y"><p class="del">gone</p><p>keep</p></div>
</body></html>"#;

	// Invalid UTF-8 (0xFF) triggers read_string's Utf8 error; the host side uniformly reports InvalidString
	fn invalid_utf8(h: &mut Harness) -> (u32, u32) {
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
		assert_eq!(set_attr(&mut h.ctx, el, kp, kl, vp, vl), 0);
		let attr_rid = h.attr_of(el, "href");
		assert_eq!(h.stored_string(attr_rid), "/x");

		// An existing key overwrites the old value
		let (vp, vl) = h.write_str("/y");
		assert_eq!(set_attr(&mut h.ctx, el, kp, kl, vp, vl), 0);
		let attr_rid = h.attr_of(el, "href");
		assert_eq!(h.stored_string(attr_rid), "/y");

		assert_eq!(remove_attr(&mut h.ctx, el, kp, kl), 0);
		assert_eq!(h.attr_of(el, "href"), -5);
		// jsoup removeAttr succeeds on a missing attribute too
		assert_eq!(remove_attr(&mut h.ctx, el, kp, kl), 0);
	}

	#[test]
	fn set_attr_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (kp, kl) = h.write_str("href");
		let (vp, vl) = h.write_str("/x");

		// Positive anchor: error-code assertions must rest on the function being implemented (a stub returning -1 for everything would fake green)
		assert_eq!(set_attr(&mut h.ctx, el, kp, kl, vp, vl), 0);
		// Invalid descriptor
		assert_eq!(set_attr(&mut h.ctx, 9999, kp, kl, vp, vl), -1);
		assert_eq!(remove_attr(&mut h.ctx, 9999, kp, kl), -1);
		// A document is not an element; attr operations report InvalidDescriptor
		assert_eq!(set_attr(&mut h.ctx, doc, kp, kl, vp, vl), -1);
		assert_eq!(remove_attr(&mut h.ctx, doc, kp, kl), -1);
		// Invalid string (illegal UTF-8)
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(set_attr(&mut h.ctx, el, bp, bl, vp, vl), -2);
		assert_eq!(set_attr(&mut h.ctx, el, kp, kl, bp, bl), -2);
		assert_eq!(remove_attr(&mut h.ctx, el, bp, bl), -2);
	}

	#[test]
	fn set_text_replaces_children() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let p = h.select_first(doc, "p.del");
		assert!(p > 0);

		let (tp, tl) = h.write_str("new & <b>");
		assert_eq!(set_text(&mut h.ctx, p, tp, tl), 0);
		assert_eq!(h.text_of(p), "new & <b>");
		assert_eq!(h.outer_html_of(p), r#"<p class="del">new &amp; &lt;b&gt;</p>"#);
		// Other queries on the same document see the mutation
		let again = h.select_first(doc, "p.del");
		assert_eq!(h.text_of(again), "new & <b>");
	}

	#[test]
	fn set_text_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (tp, tl) = h.write_str("x");

		// Positive anchor, guarding against stub fake-green
		assert_eq!(set_text(&mut h.ctx, el, tp, tl), 0);
		assert_eq!(set_text(&mut h.ctx, 9999, tp, tl), -1);
		assert_eq!(set_text(&mut h.ctx, doc, tp, tl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(set_text(&mut h.ctx, el, bp, bl), -2);
	}

	#[test]
	fn set_html_replaces_inner() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");

		let (hp, hl) = h.write_str(r#"<span class="s">hi</span> tail"#);
		assert_eq!(set_html(&mut h.ctx, el, hp, hl), 0);
		assert_eq!(h.html_of(el), r#"<span class="s">hi</span> tail"#);
		// The parsed fragment consists of real nodes and can be selected further
		let span = h.select_first(el, "span.s");
		assert!(span > 0);
		assert_eq!(h.text_of(span), "hi");
		// The old child nodes have been replaced
		assert_eq!(h.select_first(el, "p.del"), -5);
	}

	#[test]
	fn set_html_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (hp, hl) = h.write_str("<b>x</b>");

		// Positive anchor, guarding against stub fake-green
		assert_eq!(set_html(&mut h.ctx, el, hp, hl), 0);
		assert_eq!(set_html(&mut h.ctx, 9999, hp, hl), -1);
		assert_eq!(set_html(&mut h.ctx, doc, hp, hl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(set_html(&mut h.ctx, el, bp, bl), -2);
	}

	#[test]
	fn prepend_and_append_splice_fragments() {
		let mut h = Harness::new();
		let doc = h.parse_doc(r#"<!DOCTYPE html><html><body><div id="a"><p>mid</p></div></body></html>"#);
		let el = h.select_first(doc, "#a");

		let (p1, l1) = h.write_str("<b>first</b>");
		assert_eq!(prepend(&mut h.ctx, el, p1, l1), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p>");

		let (p2, l2) = h.write_str("<i>last</i>");
		assert_eq!(append(&mut h.ctx, el, p2, l2), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p><i>last</i>");

		// A multi-node fragment keeps its relative order
		let (p3, l3) = h.write_str("t1<s>1</s>t2");
		assert_eq!(append(&mut h.ctx, el, p3, l3), 0);
		assert_eq!(h.html_of(el), "<b>first</b><p>mid</p><i>last</i>t1<s>1</s>t2");

		let (p4, l4) = h.write_str("h1<u>2</u>");
		assert_eq!(prepend(&mut h.ctx, el, p4, l4), 0);
		assert_eq!(h.html_of(el), "h1<u>2</u><b>first</b><p>mid</p><i>last</i>t1<s>1</s>t2");
	}

	#[test]
	fn prepend_append_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (hp, hl) = h.write_str("<b>x</b>");

		// Positive anchor, guarding against stub fake-green
		assert_eq!(prepend(&mut h.ctx, el, hp, hl), 0);
		assert_eq!(append(&mut h.ctx, el, hp, hl), 0);
		assert_eq!(prepend(&mut h.ctx, 9999, hp, hl), -1);
		assert_eq!(append(&mut h.ctx, 9999, hp, hl), -1);
		assert_eq!(prepend(&mut h.ctx, doc, hp, hl), -1);
		assert_eq!(append(&mut h.ctx, doc, hp, hl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(prepend(&mut h.ctx, el, bp, bl), -2);
		assert_eq!(append(&mut h.ctx, el, bp, bl), -2);
	}

	#[test]
	fn add_class_and_remove_class() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		assert_eq!(h.class_name_of(el), "x y");

		let (cp, cl) = h.write_str("z");
		assert_eq!(add_class(&mut h.ctx, el, cp, cl), 0);
		assert_eq!(h.class_name_of(el), "x y z");
		assert_eq!(has_class(&mut h.ctx, el, cp, cl), 1);
		// Adding again is idempotent (jsoup set semantics)
		assert_eq!(add_class(&mut h.ctx, el, cp, cl), 0);
		assert_eq!(h.class_name_of(el), "x y z");
		// The new class is visible to later selects
		assert!(h.select_first(doc, "#a.z") > 0);

		let (yp, yl) = h.write_str("y");
		assert_eq!(remove_class(&mut h.ctx, el, yp, yl), 0);
		assert_eq!(h.class_name_of(el), "x z");
		// Removing a nonexistent class is also a success
		assert_eq!(remove_class(&mut h.ctx, el, yp, yl), 0);
		assert_eq!(h.class_name_of(el), "x z");
		assert!(h.select_first(doc, "#a.y") == -5);
	}

	#[test]
	fn class_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let (cp, cl) = h.write_str("z");

		// Positive anchor, guarding against stub fake-green
		assert_eq!(add_class(&mut h.ctx, el, cp, cl), 0);
		assert_eq!(remove_class(&mut h.ctx, el, cp, cl), 0);
		assert_eq!(add_class(&mut h.ctx, 9999, cp, cl), -1);
		assert_eq!(remove_class(&mut h.ctx, 9999, cp, cl), -1);
		assert_eq!(add_class(&mut h.ctx, doc, cp, cl), -1);
		assert_eq!(remove_class(&mut h.ctx, doc, cp, cl), -1);
		let (bp, bl) = invalid_utf8(&mut h);
		assert_eq!(add_class(&mut h.ctx, el, bp, bl), -2);
		assert_eq!(remove_class(&mut h.ctx, el, bp, bl), -2);
	}

	#[test]
	fn remove_detaches_element() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let p = h.select_first(doc, "p.del");
		assert!(p > 0);

		assert_eq!(remove(&mut h.ctx, p), 0);
		// No longer found in the document
		let list = h.select(doc, "p");
		assert!(list > 0);
		assert_eq!(size(&mut h.ctx, list), 1);
		let remaining = h.select_first(doc, "p");
		assert_eq!(h.text_of(remaining), "keep");
		// The detached element itself stays readable/writable (ego-tree detach keeps the subtree; aligned with jsoup, where the element is still usable after remove)
		assert_eq!(h.text_of(p), "gone");
		assert_eq!(parent(&mut h.ctx, p), -5);
	}

	#[test]
	fn remove_element_list() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let list = h.select(doc, "p");
		assert!(list > 0);
		assert_eq!(size(&mut h.ctx, list), 2);

		assert_eq!(remove(&mut h.ctx, list), 0);
		let after = h.select(doc, "p");
		assert!(after > 0);
		assert_eq!(size(&mut h.ctx, after), 0);
	}

	#[test]
	fn remove_error_codes() {
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");

		// Positive anchor, guarding against stub fake-green
		assert_eq!(remove(&mut h.ctx, el), 0);
		assert_eq!(remove(&mut h.ctx, 9999), -1);
		// A document is not an Element/ElementList
		assert_eq!(remove(&mut h.ctx, doc), -1);
	}

	#[test]
	fn select_empty_result_stores_empty_list_on_all_roots() {
		// F (issue #83): the Aidoku host's select does not distinguish Document/Element,
		// and SwiftSoup returns empty Elements on no match — an empty result on all three
		// roots stores an empty list; only an invalid query errors
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		assert!(el > 0);
		let list = h.select(doc, "p");
		assert!(list > 0);

		let empty = h.select(el, "span.missing");
		assert!(empty > 0, "element 根空结果应存空列表，得到 {empty}");
		assert_eq!(size(&mut h.ctx, empty), 0);

		let empty = h.select(list, "span.missing");
		assert!(empty > 0, "list 根空结果应存空列表，得到 {empty}");
		assert_eq!(size(&mut h.ctx, empty), 0);
	}

	#[test]
	fn select_empty_result_document_arm_and_error_codes() {
		// pin: an empty result on the document root has always stored an empty list; an
		// invalid query uniformly reports InvalidQuery on all three roots; select_first
		// with no match still reports NoResult (jsoup selectFirst → null semantics, outside
		// the scope of F)
		let mut h = Harness::new();
		let doc = h.parse_doc(DOC);
		let el = h.select_first(doc, "#a");
		let list = h.select(doc, "p");

		let empty = h.select(doc, "span.missing");
		assert!(empty > 0);
		assert_eq!(size(&mut h.ctx, empty), 0);

		for (root, name) in [(doc, "document"), (el, "element"), (list, "list")] {
			let (p, l) = h.write_str("p:contains(unclosed");
			assert_eq!(select(&mut h.ctx, root, p, l), -4, "{name} 根无效查询");
		}

		assert_eq!(h.select_first(doc, "span.missing"), -5);
		assert_eq!(h.select_first(el, "span.missing"), -5);
	}
}
