use ego_tree::NodeId;
use scraper::{CaseSensitivity, ElementRef, Html, Selector};
use std::{ops::Deref, rc::Rc};
use url::Url;

#[derive(Debug, Clone)]
pub struct HtmlDocument {
	pub html: Rc<Html>,
	pub base_uri: Option<Rc<Url>>,
}

#[derive(Debug, Clone)]
pub struct HtmlElement(pub HtmlNode);

#[derive(Debug, Clone)]
pub struct HtmlNode {
	pub html: Rc<Html>,
	pub base_uri: Option<Rc<Url>>,
	pub id: NodeId,
}

#[derive(Debug, Clone)]
pub struct HtmlElementList(pub Vec<HtmlElement>);

impl HtmlDocument {
	pub fn parse(html: &str, base_uri: Option<&str>) -> Self {
		let html = Html::parse_document(html);
		let base_uri = base_uri.and_then(|s| Url::parse(s).ok()).map(Rc::new);
		Self {
			html: Rc::new(html),
			base_uri,
		}
	}

	pub fn parse_fragment(html: &str, base_uri: Option<&str>) -> Self {
		let html = Html::parse_fragment(html);
		let base_uri = base_uri.and_then(|s| Url::parse(s).ok()).map(Rc::new);
		Self {
			html: Rc::new(html),
			base_uri,
		}
	}

	pub fn select(&self, selector: &Selector) -> HtmlElementList {
		let elements: Vec<HtmlElement> = self
			.html
			.select_with_root(selector)
			.map(|element| {
				HtmlElement(HtmlNode {
					html: self.html.clone(),
					base_uri: self.base_uri.clone(),
					id: element.id(),
				})
			})
			.collect();

		HtmlElementList(elements)
	}
}

impl HtmlNode {
	pub fn is_element(&self) -> bool {
		let Some(node) = self.html.tree.get(self.id) else {
			return false;
		};
		node.value().is_element()
	}

	pub fn is_document(&self) -> bool {
		let Some(node) = self.html.tree.get(self.id) else {
			return false;
		};
		node.value().is_document()
	}

	pub fn is_text(&self) -> bool {
		let Some(node) = self.html.tree.get(self.id) else {
			return false;
		};
		node.value().is_text()
	}

	pub fn is_comment(&self) -> bool {
		let Some(node) = self.html.tree.get(self.id) else {
			return false;
		};
		node.value().is_comment()
	}

	pub fn child_nodes(&self) -> Option<Vec<HtmlNode>> {
		let node = self.html.tree.get(self.id)?;
		Some(
			node.children()
				.map(|node| self.child_node(node.id()))
				.collect::<Vec<HtmlNode>>(),
		)
	}

	pub fn parent(&self) -> Option<HtmlNode> {
		let node = self.html.tree.get(self.id)?;
		node.parent().map(|node| self.child_node(node.id()))
	}

	pub fn siblings(&self) -> Option<Vec<HtmlNode>> {
		let node = self.html.tree.get(self.id)?;
		Some(
			node.next_siblings()
				.map(|node| self.child_node(node.id()))
				.collect::<Vec<HtmlNode>>(),
		)
	}

	pub fn next_sibling(&self) -> Option<HtmlNode> {
		let node = self.html.tree.get(self.id)?;
		node.next_sibling().map(|node| self.child_node(node.id()))
	}

	pub fn prev_sibling(&self) -> Option<HtmlNode> {
		let node = self.html.tree.get(self.id)?;
		node.prev_sibling().map(|node| self.child_node(node.id()))
	}

	pub fn text(&self) -> Option<String> {
		let node = self.html.tree.get(self.id)?;
		if let Some(text) = node.value().as_text() {
			Some(text.deref().into())
		} else {
			None
		}
	}

	fn child_node(&self, id: NodeId) -> HtmlNode {
		HtmlNode {
			html: self.html.clone(),
			base_uri: self.base_uri.clone(),
			id,
		}
	}
}

impl HtmlElement {
	pub fn select(&self, selector: &Selector) -> Option<HtmlElementList> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;

		let elements: Vec<HtmlElement> = element
			.select_with_root(selector)
			.map(|element| self.child(element.id()))
			.collect();

		Some(HtmlElementList(elements))
	}

	pub fn select_first(&self, selector: &Selector) -> Option<HtmlElement> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;

		element
			.select_with_root(selector)
			.next()
			.map(|element| self.child(element.id()))
	}

	/// 仅后代匹配，不含元素自身（:contains 分段求值里剩余选择器的作用域）
	pub fn select_descendants(&self, selector: &Selector) -> HtmlElementList {
		let Some(node) = self.0.html.tree.get(self.0.id) else {
			return HtmlElementList(Vec::new());
		};
		let Some(element) = ElementRef::wrap(node) else {
			return HtmlElementList(Vec::new());
		};
		HtmlElementList(
			element
				.select(selector)
				.map(|element| self.child(element.id()))
				.collect(),
		)
	}

	pub fn attr(&self, name: &str) -> Option<String> {
		let has_abs_prefix = name.starts_with("abs:");
		let name = if has_abs_prefix { &name[4..] } else { name };
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		let attr = element.attr(name).map(|value| value.to_string());
		if has_abs_prefix {
			if let Some(base_uri) = self.0.base_uri.as_ref() {
				attr.as_ref()
					// if the attribute is already a url, return it
					.and_then(|value| Url::parse(value).ok())
					.map(|url| url.to_string())
					.or_else(|| {
						// otherwise, try to join it with the base uri
						attr.as_ref()
							.and_then(|value| base_uri.join(value.as_str()).ok())
							.map(|value| value.to_string())
					})
			} else {
				attr
			}
		} else {
			attr
		}
	}

	pub fn text(&self, trimmed: bool) -> Option<String> {
		let result = ElementRef::wrap(self.0.html.tree.get(self.0.id)?)?
			.text()
			.collect::<String>();
		if trimmed {
			Some(result.trim().to_string())
		} else {
			Some(result)
		}
	}

	pub fn html(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.inner_html())
	}

	pub fn outer_html(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.html())
	}

	pub fn parent(&self) -> Option<HtmlElement> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element.parent().map(|element| self.child(element.id()))
	}

	pub fn children(&self) -> Option<HtmlElementList> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(HtmlElementList(
			element
				.child_elements()
				.map(|element| self.child(element.id()))
				.collect::<Vec<HtmlElement>>(),
		))
	}

	pub fn siblings(&self) -> Option<HtmlElementList> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(HtmlElementList(
			element
				.next_siblings()
				.map(|element| self.child(element.id()))
				.collect::<Vec<HtmlElement>>(),
		))
	}

	pub fn next_sibling(&self) -> Option<HtmlElement> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element
			.next_sibling()
			.map(|element| self.child(element.id()))
	}

	pub fn prev_sibling(&self) -> Option<HtmlElement> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element
			.prev_sibling()
			.map(|element| self.child(element.id()))
	}

	pub fn own_text(&self) -> Option<String> {
		ElementRef::wrap(self.0.html.tree.get(self.0.id)?)?
			.text()
			.next()
			.map(|text| text.to_string())
	}

	/// jsoup 仅 script/style 内容算 DataNode；html5ever 统一存文本节点，
	/// 其他元素按 jsoup 语义给空串
	pub fn data(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		if !matches!(element.value().name(), "script" | "style") {
			return Some(String::new());
		}
		Some(element.text().collect())
	}

	pub fn id(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element.value().id().map(|s| s.to_string())
	}

	pub fn tag_name(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.value().name().to_string())
	}

	pub fn class_name(&self) -> Option<String> {
		let node = self.0.html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.value().classes().collect::<Vec<&str>>().join(" "))
	}

	pub fn has_class(&self, class: &str) -> bool {
		let Some(node) = self.0.html.tree.get(self.0.id) else {
			return false;
		};
		let Some(element) = ElementRef::wrap(node) else {
			return false;
		};
		element
			.value()
			.has_class(class, CaseSensitivity::AsciiCaseInsensitive)
	}

	pub fn has_attr(&self, name: &str) -> bool {
		let Some(node) = self.0.html.tree.get(self.0.id) else {
			return false;
		};
		let Some(element) = ElementRef::wrap(node) else {
			return false;
		};
		element.value().attrs().any(|(k, _)| k == name)
	}

	pub fn has_any_attribute(&self) -> bool {
		let Some(node) = self.0.html.tree.get(self.0.id) else {
			return false;
		};
		let Some(element) = ElementRef::wrap(node) else {
			return false;
		};
		element.value().attrs().next().is_some()
	}

	fn child(&self, id: NodeId) -> HtmlElement {
		HtmlElement(self.child_node(id))
	}

	fn child_node(&self, id: NodeId) -> HtmlNode {
		HtmlNode {
			html: self.0.html.clone(),
			base_uri: self.0.base_uri.clone(),
			id,
		}
	}
}

pub enum ConversionError {
	NotElement,
}

impl TryFrom<HtmlNode> for HtmlElement {
	type Error = ConversionError;

	fn try_from(value: HtmlNode) -> Result<Self, Self::Error> {
		if value.is_element() {
			Ok(Self(value))
		} else {
			Err(ConversionError::NotElement)
		}
	}
}

impl HtmlElementList {
	pub fn select(&self, selector: &Selector) -> Option<HtmlElementList> {
		let elements: Vec<HtmlElement> = self
			.0
			.iter()
			.filter_map(|element| element.select(selector).map(|e| e.0))
			.flatten()
			.collect();
		Some(HtmlElementList(elements))
	}

	pub fn select_first(&self, selector: &Selector) -> Option<HtmlElement> {
		self.0
			.iter()
			.filter_map(|element| element.select(selector).map(|e| e.0))
			.flatten()
			.next()
	}

	pub fn attr(&self, name: &str) -> Option<String> {
		self.0
			.iter()
			.filter_map(|element| element.attr(name))
			.next()
	}

	pub fn text(&self, trimmed: bool) -> Option<String> {
		Some(
			self.0
				.iter()
				.filter_map(|element| element.text(trimmed))
				.collect::<Vec<String>>()
				.join(" "),
		)
	}

	pub fn html(&self) -> Option<String> {
		Some(
			self.0
				.iter()
				.filter_map(|element| element.html())
				.collect::<Vec<String>>()
				.join("\n"),
		)
	}

	pub fn outer_html(&self) -> Option<String> {
		Some(
			self.0
				.iter()
				.filter_map(|element| element.outer_html())
				.collect::<Vec<String>>()
				.join("\n"),
		)
	}
}

pub enum QueryRoot<'a> {
	Document(&'a HtmlDocument),
	Element(&'a HtmlElement),
	List(&'a HtmlElementList),
}

/// 字符串形式的选择器入口，额外支持 SwiftSoup 系非标准伪类：
/// :contains(text)/:containsOwn(text)/:has(selector)/[*]（含 :not 包裹）。
/// scraper 不支持这些，拆成「基础选择器 + 逐项过滤 + 剩余选择器（候选元素
/// 作用域、不含自身）」分段递归求值；后缀的 > 按后代处理（真实源写法都是
/// li:contains(x) > span 这类，极端嵌套下结果是超集，总比整体失败强）
pub fn select_str(root: QueryRoot, query: &str) -> Result<HtmlElementList, ()> {
	select_str_impl(root, query, true)
}

fn select_str_impl(root: QueryRoot, query: &str, include_root: bool) -> Result<HtmlElementList, ()> {
	if let Some(split) = split_extension(query) {
		let prefix = split.prefix.trim();
		let prefix = if prefix.is_empty() { "*" } else { prefix };
		let base = select_str_impl(root, prefix, include_root)?;
		let mut filtered = Vec::new();
		for element in base.0 {
			if matches_extension(&element, &split.filter)? {
				filtered.push(element);
			}
		}
		let remainder = split.suffix.trim_start();
		if remainder.is_empty() {
			return Ok(HtmlElementList(filtered));
		}
		let remainder = remainder
			.strip_prefix('>')
			.map(str::trim_start)
			.unwrap_or(remainder);
		if remainder.is_empty() || remainder.starts_with('+') || remainder.starts_with('~') {
			return Err(());
		}
		let mut out = Vec::new();
		for element in &filtered {
			out.extend(select_str_impl(QueryRoot::Element(element), remainder, false)?.0);
		}
		return Ok(HtmlElementList(out));
	}
	let selector = Selector::parse(query).map_err(|_| ())?;
	let list = match root {
		QueryRoot::Document(doc) => doc.select(&selector),
		QueryRoot::Element(element) => {
			if include_root {
				element
					.select(&selector)
					.unwrap_or_else(|| HtmlElementList(Vec::new()))
			} else {
				element.select_descendants(&selector)
			}
		}
		QueryRoot::List(list) => {
			let mut out = Vec::new();
			for element in &list.0 {
				if include_root {
					out.extend(element.select(&selector).map(|l| l.0).unwrap_or_default());
				} else {
					out.extend(element.select_descendants(&selector).0);
				}
			}
			HtmlElementList(out)
		}
	};
	Ok(list)
}

enum ExtensionFilter {
	Contains { needle: String, own: bool, negate: bool },
	Has { selectors: Vec<String>, negate: bool },
	AttrWildcard { negate: bool },
}

struct ExtensionSplit {
	prefix: String,
	filter: ExtensionFilter,
	suffix: String,
}

#[derive(Clone, Copy)]
enum MarkerKind {
	Contains { own: bool, negate: bool },
	Has { negate: bool },
	Attr { negate: bool },
}

/// 切出第一个 scraper 不支持的伪类（:contains/:containsOwn/:has/[*]，含 :not 包裹）。
/// 找到标记但括号不闭合时返回 None，让外层走 Selector::parse 报无效查询
fn split_extension(query: &str) -> Option<ExtensionSplit> {
	const PAREN_MARKERS: &[(&str, MarkerKind)] = &[
		(":not(:containsOwn(", MarkerKind::Contains { own: true, negate: true }),
		(":not(:contains(", MarkerKind::Contains { own: false, negate: true }),
		(":containsOwn(", MarkerKind::Contains { own: true, negate: false }),
		(":contains(", MarkerKind::Contains { own: false, negate: false }),
		(":not(:has(", MarkerKind::Has { negate: true }),
		(":has(", MarkerKind::Has { negate: false }),
		(":not([*])", MarkerKind::Attr { negate: true }),
		("[*]", MarkerKind::Attr { negate: false }),
	];
	let mut found: Option<(usize, &'static str, MarkerKind)> = None;
	for (marker, kind) in PAREN_MARKERS {
		if let Some(index) = query.find(marker)
			&& found.is_none_or(|(i, _, _)| index < i)
		{
			found = Some((index, marker, *kind));
		}
	}
	let (index, marker, kind) = found?;
	let prefix = query[..index].to_string();
	if let MarkerKind::Attr { negate } = kind {
		return Some(ExtensionSplit {
			prefix,
			filter: ExtensionFilter::AttrWildcard { negate },
			suffix: query[index + marker.len()..].to_string(),
		});
	}
	let extra_depth = match kind {
		MarkerKind::Contains { negate: true, .. } | MarkerKind::Has { negate: true } => 1,
		_ => 0,
	};
	let (content, suffix) = take_paren_content(query, index + marker.len(), extra_depth)?;
	match kind {
		MarkerKind::Contains { own, negate } => Some(ExtensionSplit {
			prefix,
			filter: ExtensionFilter::Contains {
				needle: unquote(content.trim()),
				own,
				negate,
			},
			suffix,
		}),
		MarkerKind::Has { negate } => {
			let selectors = split_top_level_commas(&content);
			if selectors.iter().any(|s| s.is_empty()) {
				return None;
			}
			Some(ExtensionSplit {
				prefix,
				filter: ExtensionFilter::Has { selectors, negate },
				suffix,
			})
		}
		MarkerKind::Attr { .. } => unreachable!(),
	}
}

/// 从 start 扫描到配平后的右括号（extra_depth 抵消 :not 多包的一层），引号感知。
/// 返回（内容, 剩余后缀）；extra_depth>0 时内容裁到内层闭合处
fn take_paren_content(query: &str, start: usize, extra_depth: u8) -> Option<(String, String)> {
	let rest = &query[start..];
	let mut quote: Option<char> = None;
	let mut depth = extra_depth;
	let mut inner_end = None;
	for (i, c) in rest.char_indices() {
		match quote {
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == '(' => depth += 1,
			None if c == ')' => {
				if depth == 0 {
					let end = inner_end.unwrap_or(i);
					return Some((rest[..end].to_string(), rest[i + 1..].to_string()));
				}
				depth -= 1;
				if depth == 0 && extra_depth > 0 {
					inner_end = Some(i);
				}
			}
			None => {}
		}
	}
	None
}

fn split_top_level_commas(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut quote: Option<char> = None;
	let mut depth = 0u8;
	let mut start = 0;
	for (i, c) in text.char_indices() {
		match quote {
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == '(' => depth += 1,
			None if c == ')' => depth = depth.saturating_sub(1),
			None if c == ',' && depth == 0 => {
				out.push(text[start..i].trim().to_string());
				start = i + 1;
			}
			None => {}
		}
	}
	out.push(text[start..].trim().to_string());
	out
}

fn unquote(raw: &str) -> String {
	if raw.len() >= 2
		&& ((raw.starts_with('"') && raw.ends_with('"'))
			|| (raw.starts_with('\'') && raw.ends_with('\'')))
	{
		raw[1..raw.len() - 1].to_string()
	} else {
		raw.to_string()
	}
}

fn matches_extension(element: &HtmlElement, filter: &ExtensionFilter) -> Result<bool, ()> {
	Ok(match filter {
		ExtensionFilter::Contains { needle, own, negate } => {
			contains_text(element, needle, *own) != *negate
		}
		ExtensionFilter::AttrWildcard { negate } => element.has_any_attribute() != *negate,
		ExtensionFilter::Has { selectors, negate } => {
			// jsoup :has 是后代相对选择器（不含自身），子选择器可再嵌套扩展伪类
			let mut found = false;
			for sub in selectors {
				if !select_str_impl(QueryRoot::Element(element), sub, false)?.0.is_empty() {
					found = true;
					break;
				}
			}
			found != *negate
		}
	})
}

/// SwiftSoup :contains 是子树文本、大小写不敏感、空白折叠后匹配
fn contains_text(element: &HtmlElement, needle: &str, own: bool) -> bool {
	let text = if own {
		element.own_text()
	} else {
		element.text(false)
	};
	let Some(text) = text else { return false };
	normalize_whitespace(&text).contains(&normalize_whitespace(needle))
}

fn normalize_whitespace(text: &str) -> String {
	text.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ")
		.to_lowercase()
}

pub trait SelectWithRoot<'a, 'b> {
	fn select_with_root(&'a self, selector: &'b Selector) -> SelectRoot<'a, 'b>;
}

pub struct SelectRoot<'a, 'b> {
	root: Option<std::iter::Once<ElementRef<'a>>>,
	select_el: Option<scraper::element_ref::Select<'a, 'b>>,
	select_doc: Option<scraper::html::Select<'a, 'b>>,
}

impl<'a> Iterator for SelectRoot<'a, '_> {
	type Item = ElementRef<'a>;

	fn next(&mut self) -> Option<Self::Item> {
		if self.root.is_some() {
			self.root.take().unwrap().next()
		} else if let Some(select) = self.select_el.as_mut() {
			select.next()
		} else if let Some(select) = self.select_doc.as_mut() {
			select.next()
		} else {
			None
		}
	}
}

impl<'a, 'b> SelectWithRoot<'a, 'b> for Html {
	fn select_with_root(&'a self, selector: &'b Selector) -> SelectRoot<'a, 'b> {
		let root_el = self.tree.root();

		let root = if let Some(element) = ElementRef::wrap(root_el)
			&& selector.matches(&element)
		{
			Some(std::iter::once(element))
		} else {
			None
		};

		let select = self.select(selector);

		SelectRoot {
			root,
			select_el: None,
			select_doc: Some(select),
		}
	}
}

impl<'a, 'b> SelectWithRoot<'a, 'b> for ElementRef<'a> {
	fn select_with_root(&'a self, selector: &'b Selector) -> SelectRoot<'a, 'b> {
		let root = if selector.matches(self) {
			Some(std::iter::once(*self))
		} else {
			None
		};

		let select = self.select(selector);

		SelectRoot {
			root,
			select_el: Some(select),
			select_doc: None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::imports::html::{escape_str, unescape_str};

	const HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<div id="content" class="main">
<p class="text">  Hello &amp; goodbye  </p>
<p class="text">Second</p>
<a href="/path/to/page">link</a>
<img src="https://example.com/img.png">
</div>
</body>
</html>"#;

	fn content_element(doc: &HtmlDocument) -> HtmlElement {
		doc.select(&Selector::parse("#content").unwrap())
			.0
			.into_iter()
			.next()
			.unwrap()
	}

	#[test]
	fn parse_and_select() {
		let doc = HtmlDocument::parse(HTML, Some("https://example.com/dir/index.html"));
		assert!(doc.base_uri.is_some());

		let list = doc.select(&Selector::parse("p.text").unwrap());
		assert_eq!(list.0.len(), 2);

		// 无法解析为 URL 的 base 会被丢弃
		let doc = HtmlDocument::parse(HTML, Some("not a url"));
		assert!(doc.base_uri.is_none());
	}

	#[test]
	fn select_first_includes_root() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);

		// select_with_root：元素自身匹配时也计入结果
		let self_match = content.select_first(&Selector::parse("div.main").unwrap()).unwrap();
		assert_eq!(self_match.id().as_deref(), Some("content"));

		let first_p = content.select_first(&Selector::parse("p.text").unwrap()).unwrap();
		assert_eq!(first_p.text(true).as_deref(), Some("Hello & goodbye"));
	}

	#[test]
	fn text_variants() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);
		let first_p = content.select_first(&Selector::parse("p.text").unwrap()).unwrap();

		assert_eq!(first_p.text(true).as_deref(), Some("Hello & goodbye"));
		assert_eq!(first_p.text(false).as_deref(), Some("  Hello & goodbye  "));

		let nodes = first_p.0.child_nodes().unwrap();
		assert_eq!(nodes.len(), 1);
		assert!(nodes[0].is_text());
		assert_eq!(nodes[0].text().as_deref(), Some("  Hello & goodbye  "));

		let list = doc.select(&Selector::parse("p.text").unwrap());
		assert_eq!(list.text(true).as_deref(), Some("Hello & goodbye Second"));
	}

	#[test]
	fn attr_with_abs_prefix() {
		let doc = HtmlDocument::parse(HTML, Some("https://example.com/dir/index.html"));
		let content = content_element(&doc);

		let link = content.select_first(&Selector::parse("a").unwrap()).unwrap();
		assert_eq!(link.attr("href").as_deref(), Some("/path/to/page"));
		assert_eq!(
			link.attr("abs:href").as_deref(),
			Some("https://example.com/path/to/page")
		);
		assert_eq!(link.attr("missing"), None);

		// 已是绝对 URL 的属性原样返回
		let img = content.select_first(&Selector::parse("img").unwrap()).unwrap();
		assert_eq!(
			img.attr("abs:src").as_deref(),
			Some("https://example.com/img.png")
		);
	}

	#[test]
	fn outer_html_and_inner_html() {
		let doc = HtmlDocument::parse(HTML, None);
		let second = doc
			.select(&Selector::parse("p.text").unwrap())
			.0
			.into_iter()
			.nth(1)
			.unwrap();
		assert_eq!(
			second.outer_html().as_deref(),
			Some(r#"<p class="text">Second</p>"#)
		);
		assert_eq!(second.html().as_deref(), Some("Second"));
	}

	#[test]
	fn escape_and_unescape() {
		assert_eq!(escape_str("a & b <c>"), "a &amp; b &lt;c&gt;");
		assert_eq!(escape_str("&amp;"), "&amp;amp;");

		assert_eq!(unescape_str("&lt;div&gt;&amp;&lt;/div&gt;"), "<div>&</div>");
		assert_eq!(unescape_str("&unknown; &amp"), "&unknown; &amp");

		let raw = "Tom & Jerry <3 >2";
		assert_eq!(unescape_str(&escape_str(raw)), raw);
	}

	#[test]
	fn contains_basic_and_scoped_suffix() {
		let html = r#"<!DOCTYPE html><html><body>
		<ul>
		<li>狀態： <span class="txt">連載中</span></li>
		<li>作者： <span class="txt">某人</span></li>
		</ul>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(QueryRoot::Document(&doc), "li:contains(狀態：)").unwrap();
		assert_eq!(list.0.len(), 1);

		let list = select_str(QueryRoot::Document(&doc), "li:contains(狀態：) > span.txt").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("連載中"));

		// 大小写不敏感；引号包裹的 needle 里可以有右括号
		let html = r#"<!DOCTYPE html><html><body><p>Hello)a World</p></body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let list = select_str(QueryRoot::Document(&doc), r#"p:contains("hello)a")"#).unwrap();
		assert_eq!(list.0.len(), 1);
	}

	#[test]
	fn contains_excludes_scope_root() {
		let html = r#"<!DOCTYPE html><html><body>
		<div class="y">x<div class="y">z</div></div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let list = select_str(QueryRoot::Document(&doc), "div:contains(x) div.y").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("z"));
	}

	#[test]
	fn contains_invalid_query() {
		let doc = HtmlDocument::parse("<p>x</p>", None);
		assert!(select_str(QueryRoot::Document(&doc), "li:contains(unclosed").is_err());
		assert!(select_str(QueryRoot::Document(&doc), "p:contains(x) + p").is_err());
	}

	#[test]
	fn attr_wildcard() {
		let html = r#"<!DOCTYPE html><html><body>
		<script>var x = 1;</script>
		<script src="https://example.com/a.js"></script>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let list = select_str(QueryRoot::Document(&doc), "script:not([*])").unwrap();
		assert_eq!(list.0.len(), 1);
		assert!(list.0[0].text(false).unwrap().contains("var x"));

		let list = select_str(QueryRoot::Document(&doc), "script[*]").unwrap();
		assert_eq!(list.0.len(), 1);
	}

	#[test]
	fn script_data() {
		let html = r#"<!DOCTYPE html><html><body>
		<script>var contentKey = 'abc';</script>
		<div>text</div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let script = select_str(QueryRoot::Document(&doc), "script").unwrap();
		assert_eq!(script.0[0].data().as_deref(), Some("var contentKey = 'abc';"));

		let div = select_str(QueryRoot::Document(&doc), "div").unwrap();
		assert_eq!(div.0[0].data().as_deref(), Some(""));
	}

	#[test]
	fn has_nested_and_negated() {
		let html = r#"<!DOCTYPE html><html><body>
		<section><h2>Latest Updates</h2><article>A</article></section>
		<section><h2>Other</h2><article>B</article></section>
		<p class="data">plain</p>
		<p class="data"><b>bold</b></p>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(
			QueryRoot::Document(&doc),
			"section:has(h2:contains(Latest Updates)) article",
		)
		.unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("A"));

		let list = select_str(QueryRoot::Document(&doc), "p.data:not(:has(*))").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("plain"));
	}

	#[test]
	fn has_comma_list() {
		let html = r#"<!DOCTYPE html><html><body>
		<ul>
		<li><strong>Type</strong>: manga <a href="/1">x</a></li>
		<li><strong>Other</strong> <a href="/2">y</a></li>
		</ul>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let list = select_str(
			QueryRoot::Document(&doc),
			"ul > li:has(strong:contains(Type),strong:contains(Tag)) a",
		)
		.unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].attr("href").as_deref(), Some("/1"));
	}
}
