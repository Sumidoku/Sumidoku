use ego_tree::NodeId;
use html5ever::{Attribute, LocalName, QualName, ns};
use regex::Regex;
use scraper::{CaseSensitivity, ElementRef, Html, Selector};
use scraper::node::{Node, Text};
use std::{cell::RefCell, ops::Deref, rc::Rc};
use url::Url;

#[derive(Debug, Clone)]
pub struct HtmlDocument {
	pub html: Rc<RefCell<Html>>,
	pub base_uri: Option<Rc<Url>>,
}

#[derive(Debug, Clone)]
pub struct HtmlElement(pub HtmlNode);

#[derive(Debug, Clone)]
pub struct HtmlNode {
	pub html: Rc<RefCell<Html>>,
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
			html: Rc::new(RefCell::new(html)),
			base_uri,
		}
	}

	pub fn parse_fragment(html: &str, base_uri: Option<&str>) -> Self {
		let html = Html::parse_fragment(html);
		let base_uri = base_uri.and_then(|s| Url::parse(s).ok()).map(Rc::new);
		Self {
			html: Rc::new(RefCell::new(html)),
			base_uri,
		}
	}

	pub fn select(&self, selector: &Selector) -> HtmlElementList {
		let html = self.html.borrow();
		let elements: Vec<HtmlElement> = html
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
		let html = self.html.borrow();
		let Some(node) = html.tree.get(self.id) else {
			return false;
		};
		node.value().is_element()
	}

	pub fn is_document(&self) -> bool {
		let html = self.html.borrow();
		let Some(node) = html.tree.get(self.id) else {
			return false;
		};
		node.value().is_document()
	}

	pub fn is_text(&self) -> bool {
		let html = self.html.borrow();
		let Some(node) = html.tree.get(self.id) else {
			return false;
		};
		node.value().is_text()
	}

	pub fn is_comment(&self) -> bool {
		let html = self.html.borrow();
		let Some(node) = html.tree.get(self.id) else {
			return false;
		};
		node.value().is_comment()
	}

	pub fn child_nodes(&self) -> Option<Vec<HtmlNode>> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		Some(
			node.children()
				.map(|node| self.child_node(node.id()))
				.collect::<Vec<HtmlNode>>(),
		)
	}

	pub fn parent(&self) -> Option<HtmlNode> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		node.parent().map(|node| self.child_node(node.id()))
	}

	/// jsoup siblingNodes()：父节点全部子节点排除自身、按文档序；
	/// node 层不按元素过滤，文本/注释都在
	pub fn siblings(&self) -> Option<Vec<HtmlNode>> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		let Some(parent) = node.parent() else {
			return Some(Vec::new());
		};
		Some(
			parent
				.children()
				.filter(|sibling| sibling.id() != self.id)
				.map(|sibling| self.child_node(sibling.id()))
				.collect::<Vec<HtmlNode>>(),
		)
	}

	/// 紧邻兄弟节点（文本/注释也算）：node 层不做元素过滤，元素级导航在 HtmlElement
	pub fn next_sibling(&self) -> Option<HtmlNode> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		node.next_sibling().map(|node| self.child_node(node.id()))
	}

	pub fn prev_sibling(&self) -> Option<HtmlNode> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		node.prev_sibling().map(|node| self.child_node(node.id()))
	}

	pub fn text(&self) -> Option<String> {
		let html = self.html.borrow();
		let node = html.tree.get(self.id)?;
		node.value().as_text().map(|text| text.deref().into())
	}

	/// None = 节点已不在树中。非元素节点报 None：ego-tree 只有元素挂属性，
	/// 与上游 jsoup 的 Node.attr 差异在此（上游 host 未实现该函数，无 ABI 冲突）
	pub fn set_attr(&self, key: &str, value: &str) -> Option<()> {
		self.replace_attrs(|attrs| {
			if let Some(existing) = attrs.iter_mut().find(|(k, _)| k == key) {
				existing.1 = value.to_string();
			} else {
				attrs.push((key.to_string(), value.to_string()));
			}
		})
	}

	/// jsoup removeAttr：属性不存在也是成功
	pub fn remove_attr(&self, key: &str) -> Option<()> {
		self.replace_attrs(|attrs| attrs.retain(|(k, _)| k != key))
	}

	/// jsoup Element.text(String)：清空子节点，换成单个文本节点
	pub fn set_text(&self, text: &str) -> Option<()> {
		let mut html = self.html.borrow_mut();
		let mut node = html.tree.get_mut(self.id)?;
		if !matches!(node.value(), Node::Element(_)) {
			return None;
		}
		detach_children(&mut node);
		node.append(Node::Text(Text { text: text.into() }));
		Some(())
	}

	/// jsoup Element.html(String)：解析 fragment 替换全部子节点
	pub fn set_html(&self, fragment: &str) -> Option<()> {
		self.splice(fragment, Splice::Replace)
	}

	pub fn prepend_html(&self, fragment: &str) -> Option<()> {
		self.splice(fragment, Splice::Prepend)
	}

	pub fn append_html(&self, fragment: &str) -> Option<()> {
		self.splice(fragment, Splice::Append)
	}

	/// class 读写都走 class 属性重写：jsoup 语义是「空白切分、去重保序、集合运算后写回」，
	/// removeClass 清空后保留空 class 属性（jsoup classNames(set) 恒写回）
	pub fn add_class(&self, class: &str) -> Option<()> {
		self.replace_attrs(|attrs| {
			let mut classes = normalized_classes(attrs);
			if !classes.iter().any(|c| c == class) {
				classes.push(class.to_string());
			}
			set_class_attr(attrs, classes);
		})
	}

	pub fn remove_class(&self, class: &str) -> Option<()> {
		self.replace_attrs(|attrs| {
			let mut classes = normalized_classes(attrs);
			classes.retain(|c| c != class);
			set_class_attr(attrs, classes);
		})
	}

	/// ego-tree detach 只摘父子/兄弟链接，子树完整留在 arena：
	/// 被 remove 的元素之后仍可读写（对齐 jsoup remove 后元素对象仍可用），
	/// 只是从文档根不可达
	pub fn detach(&self) -> Option<()> {
		let mut html = self.html.borrow_mut();
		html.tree.get_mut(self.id)?.detach();
		Some(())
	}

	fn splice(&self, fragment: &str, mode: Splice) -> Option<()> {
		let frag = Html::parse_fragment(fragment);
		let mut html = self.html.borrow_mut();
		// extend_tree 把 fragment 树并入（orphan 子树）；
		// html5ever fragment 算法会造一个 <html> 包裹元素，内容节点是它的子节点
		let frag_root_id = html.tree.extend_tree(frag.tree).id();
		let container_id = fragment_container(&html.tree, frag_root_id);
		let mut node = html.tree.get_mut(self.id)?;
		if !matches!(node.value(), Node::Element(_)) {
			return None;
		}
		if mode == Splice::Replace {
			detach_children(&mut node);
		}
		// reparent_from_id_* 整段移动子节点链表，fragment 内相对顺序不变
		match mode {
			Splice::Prepend => node.reparent_from_id_prepend(container_id),
			_ => node.reparent_from_id_append(container_id),
		}
		Some(())
	}

	fn replace_attrs(&self, f: impl FnOnce(&mut Vec<(String, String)>)) -> Option<()> {
		let mut html = self.html.borrow_mut();
		let mut node = html.tree.get_mut(self.id)?;
		let Node::Element(element) = node.value() else {
			return None;
		};
		// Element 的 id/classes 是 OnceCell 缓存，直接改 attrs 会让 .class/#id
		// 选择器读到旧值——必须 Element::new 整体重建让缓存失效
		let mut attrs: Vec<(String, String)> = element
			.attrs()
			.map(|(k, v)| (k.to_string(), v.to_string()))
			.collect();
		let name = element.name.clone();
		f(&mut attrs);
		*element = scraper::node::Element::new(
			name,
			attrs
				.into_iter()
				.map(|(k, v)| Attribute {
					name: QualName::new(None, ns!(), LocalName::from(k.as_str())),
					value: v.into(),
				})
				.collect(),
		);
		Some(())
	}

	fn child_node(&self, id: NodeId) -> HtmlNode {
		HtmlNode {
			html: self.html.clone(),
			base_uri: self.base_uri.clone(),
			id,
		}
	}
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Splice {
	Prepend,
	Append,
	Replace,
}

fn detach_children(node: &mut ego_tree::NodeMut<'_, Node>) {
	let children: Vec<NodeId> = node.as_ref().children().map(|child| child.id()).collect();
	for id in children {
		if let Some(mut child) = node.tree().get_mut(id) {
			child.detach();
		}
	}
}

/// fragment 树的内容容器：恰好一个 <html> 子元素时剥开它，否则直接用根
fn fragment_container(tree: &ego_tree::Tree<Node>, root_id: NodeId) -> NodeId {
	let Some(root) = tree.get(root_id) else {
		return root_id;
	};
	let mut children = root.children();
	let (Some(first), None) = (children.next(), children.next()) else {
		return root_id;
	};
	match first.value().as_element() {
		Some(element) if element.name.local.as_ref() == "html" => first.id(),
		_ => root_id,
	}
}

fn normalized_classes(attrs: &[(String, String)]) -> Vec<String> {
	let raw = attrs
		.iter()
		.find(|(k, _)| k == "class")
		.map(|(_, v)| v.as_str())
		.unwrap_or("");
	let mut classes: Vec<String> = Vec::new();
	for name in raw.split_whitespace() {
		if !classes.iter().any(|c| c == name) {
			classes.push(name.to_string());
		}
	}
	classes
}

fn set_class_attr(attrs: &mut Vec<(String, String)>, classes: Vec<String>) {
	let value = classes.join(" ");
	if let Some(existing) = attrs.iter_mut().find(|(k, _)| k == "class") {
		existing.1 = value;
	} else {
		attrs.push(("class".to_string(), value));
	}
}

impl HtmlElement {
	pub fn select(&self, selector: &Selector) -> Option<HtmlElementList> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;

		let elements: Vec<HtmlElement> = element
			.select_with_root(selector)
			.map(|element| self.child(element.id()))
			.collect();

		Some(HtmlElementList(elements))
	}

	pub fn select_first(&self, selector: &Selector) -> Option<HtmlElement> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;

		element
			.select_with_root(selector)
			.next()
			.map(|element| self.child(element.id()))
	}

	/// 仅后代匹配，不含元素自身（:contains 分段求值里剩余选择器的作用域）
	pub fn select_descendants(&self, selector: &Selector) -> HtmlElementList {
		let html = self.0.html.borrow();
		let Some(node) = html.tree.get(self.0.id) else {
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
		let attr = {
			let html = self.0.html.borrow();
			let node = html.tree.get(self.0.id)?;
			let element = ElementRef::wrap(node)?;
			element.attr(name).map(|value| value.to_string())
		};
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

	/// trimmed=true 对齐 jsoup text()：归一化 + trim；trimmed=false 是 aidoku
	/// untrimmed_text：文本节点逐字拼接、不 trim，但块级分隔与 script/style
	/// 排除照旧（vendor 文档「Notices from Element::text apply」）
	pub fn text(&self, trimmed: bool) -> Option<String> {
		let html = self.0.html.borrow();
		let element = ElementRef::wrap(html.tree.get(self.0.id)?)?;
		let text = collect_text(element, trimmed);
		if trimmed {
			Some(jsoup_trim(&text).to_string())
		} else {
			Some(text)
		}
	}

	pub fn html(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.inner_html())
	}

	pub fn outer_html(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.html())
	}

	pub fn parent(&self) -> Option<HtmlElement> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element.parent().map(|element| self.child(element.id()))
	}

	pub fn children(&self) -> Option<HtmlElementList> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(HtmlElementList(
			element
				.child_elements()
				.map(|element| self.child(element.id()))
				.collect::<Vec<HtmlElement>>(),
		))
	}

	/// jsoup siblingElements()：父元素的全部元素子节点排除自身、按文档序
	pub fn siblings(&self) -> Option<HtmlElementList> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		ElementRef::wrap(node)?;
		let Some(parent) = node.parent() else {
			return Some(HtmlElementList(Vec::new()));
		};
		Some(HtmlElementList(
			parent
				.children()
				.filter(|sibling| sibling.id() != self.0.id)
				.filter_map(ElementRef::wrap)
				.map(|sibling| self.child(sibling.id()))
				.collect::<Vec<HtmlElement>>(),
		))
	}

	/// jsoup nextElementSibling()：沿兄弟链找第一个元素节点，跳过文本/注释
	pub fn next_sibling(&self) -> Option<HtmlElement> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		let sibling = element.next_siblings().find_map(ElementRef::wrap)?;
		Some(self.child(sibling.id()))
	}

	/// jsoup previousElementSibling()：反向沿兄弟链找第一个元素节点
	pub fn prev_sibling(&self) -> Option<HtmlElement> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		let sibling = element.prev_siblings().find_map(ElementRef::wrap)?;
		Some(self.child(sibling.id()))
	}

	/// jsoup ownText()：只拼直系文本节点子节点，与 text() 同一套归一化；
	/// 直系 <br> 补一个空格；script/style 的内容是 data 不算 own text
	pub fn own_text(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let element = ElementRef::wrap(html.tree.get(self.0.id)?)?;
		let is_data_element = matches!(element.value().name(), "script" | "style");
		let mut accum = String::new();
		for child in element.children() {
			match child.value() {
				Node::Text(text) if !is_data_element => {
					append_normalised_text(&mut accum, child, text.deref())
				}
				Node::Element(child_element)
					if child_element.name() == "br" && !accum.ends_with(' ') =>
				{
					accum.push(' ');
				}
				_ => {}
			}
		}
		Some(jsoup_trim(&accum).to_string())
	}

	/// jsoup 仅 script/style 内容算 DataNode；html5ever 统一存文本节点，
	/// 其他元素按 jsoup 语义给空串
	pub fn data(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		if !matches!(element.value().name(), "script" | "style") {
			return Some(String::new());
		}
		Some(element.text().collect())
	}

	pub fn id(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		element.value().id().map(|s| s.to_string())
	}

	pub fn tag_name(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.value().name().to_string())
	}

	pub fn class_name(&self) -> Option<String> {
		let html = self.0.html.borrow();
		let node = html.tree.get(self.0.id)?;
		let element = ElementRef::wrap(node)?;
		Some(element.value().classes().collect::<Vec<&str>>().join(" "))
	}

	pub fn has_class(&self, class: &str) -> bool {
		let html = self.0.html.borrow();
		let Some(node) = html.tree.get(self.0.id) else {
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
		let html = self.0.html.borrow();
		let Some(node) = html.tree.get(self.0.id) else {
			return false;
		};
		let Some(element) = ElementRef::wrap(node) else {
			return false;
		};
		element.value().attrs().any(|(k, _)| k == name)
	}

	pub fn has_any_attribute(&self) -> bool {
		let html = self.0.html.borrow();
		let Some(node) = html.tree.get(self.0.id) else {
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
	/// 组合器（>/+/~）锁定的候选集：元素自身必须命中下一段复合选择器
	Scope(Vec<HtmlElement>),
}

/// 字符串形式的选择器入口，额外支持 SwiftSoup 系非标准伪类：
/// :contains/:containsOwn/:matches/:matchesOwn/:lt/:gt/:eq/[*]/[^前缀]，
/// 以及它们的 :not 包裹和含扩展内容的 :has。
/// 求值按「复合段 + 组合器」推进：组合器开新作用域（> 只取直接子元素，
/// +/~ 只取兄弟元素），同复合剩余作用于候选元素自身；顶层逗号分支按
/// jsoup 语义求并集（文档序、去重）。:has/:not 内容不含扩展伪类时整个
/// 查询交给 Selector::parse（selectors crate 原生支持 CSS4 相对选择器）
pub fn select_str(root: QueryRoot, query: &str) -> Result<HtmlElementList, ()> {
	select_str_impl(root, query, true)
}

fn select_str_impl(root: QueryRoot, query: &str, include_root: bool) -> Result<HtmlElementList, ()> {
	if split_extension(query).is_none() {
		return Ok(HtmlElementList(select_css(&root, query, include_root)?));
	}
	let branches = split_top_level_commas(query);
	if branches.iter().any(|branch| branch.is_empty()) {
		return Err(());
	}
	let mut out = Vec::new();
	for branch in &branches {
		out.extend(eval(&root, branch, include_root)?);
	}
	if branches.len() > 1 {
		sort_dedup_document_order(&mut out);
	}
	Ok(HtmlElementList(out))
}

/// 单分支求值：切出首段复合选择器匹配候选，再按组合器递归推进作用域
fn eval(root: &QueryRoot, query: &str, include_root: bool) -> Result<Vec<HtmlElement>, ()> {
	let (head, continuation) = split_head(query);
	let candidates = match_compound(root, head, include_root)?;
	apply_continuation(candidates, continuation)
}

/// 复合选择器（无顶层组合器）匹配：Scope 下元素自身必须命中；
/// 其余根按 include_root 决定元素自身是否参与
fn match_compound(root: &QueryRoot, compound: &str, include_root: bool) -> Result<Vec<HtmlElement>, ()> {
	let compound = compound.trim();
	if let QueryRoot::Scope(scope) = root {
		let mut out = Vec::new();
		for element in scope {
			if matches_self(element, compound)? {
				out.push(element.clone());
			}
		}
		return Ok(out);
	}
	if let Some(split) = split_extension(compound) {
		let prefix = split.prefix.trim();
		let prefix = if prefix.is_empty() { "*" } else { prefix };
		let base = match_compound(root, prefix, include_root)?;
		let rest = split.suffix.trim();
		let mut out = Vec::new();
		for element in base {
			if matches_extension(&element, &split.filter)?
				&& (rest.is_empty() || matches_self(&element, rest)?)
			{
				out.push(element);
			}
		}
		return Ok(out);
	}
	select_css(root, compound, include_root)
}

/// 同复合剩余匹配候选元素自身（扩展伪类逐段消化，纯 CSS 段交给 selectors crate）
fn matches_self(element: &HtmlElement, compound: &str) -> Result<bool, ()> {
	if let Some(split) = split_extension(compound) {
		let prefix = split.prefix.trim();
		if !prefix.is_empty() {
			let selector = Selector::parse(prefix).map_err(|_| ())?;
			if !element_matches(element, &selector) {
				return Ok(false);
			}
		}
		if !matches_extension(element, &split.filter)? {
			return Ok(false);
		}
		let rest = split.suffix.trim();
		return if rest.is_empty() { Ok(true) } else { matches_self(element, rest) };
	}
	let selector = Selector::parse(compound).map_err(|_| ())?;
	Ok(element_matches(element, &selector))
}

fn apply_continuation(candidates: Vec<HtmlElement>, continuation: &str) -> Result<Vec<HtmlElement>, ()> {
	let trimmed = continuation.trim_start();
	if trimmed.is_empty() {
		return Ok(candidates);
	}
	let rest = match trimmed.as_bytes()[0] {
		b'>' => return apply_scope(candidates, Scope::Children, trimmed[1..].trim()),
		b'+' => return apply_scope(candidates, Scope::NextSibling, trimmed[1..].trim()),
		b'~' => return apply_scope(candidates, Scope::LaterSiblings, trimmed[1..].trim()),
		_ => trimmed,
	};
	if rest.is_empty() {
		return Err(());
	}
	let mut out = Vec::new();
	for element in &candidates {
		out.extend(eval(&QueryRoot::Element(element), rest, false)?);
	}
	Ok(out)
}

enum Scope {
	Children,
	NextSibling,
	LaterSiblings,
}

fn apply_scope(candidates: Vec<HtmlElement>, scope: Scope, rest: &str) -> Result<Vec<HtmlElement>, ()> {
	if rest.is_empty() {
		// 悬空组合器（如 `p:contains(x) +`）按无效查询处理
		return Err(());
	}
	let elements: Vec<HtmlElement> = match scope {
		Scope::Children => candidates.iter().flat_map(element_children).collect(),
		Scope::NextSibling => candidates.iter().filter_map(next_element_sibling).collect(),
		Scope::LaterSiblings => {
			let mut siblings: Vec<HtmlElement> =
				candidates.iter().flat_map(following_element_siblings).collect();
			// 多个候选的后续兄弟集合互相重叠
			dedup_elements(&mut siblings);
			siblings
		}
	};
	eval(&QueryRoot::Scope(elements), rest, true)
}

fn select_css(root: &QueryRoot, query: &str, include_root: bool) -> Result<Vec<HtmlElement>, ()> {
	let selector = Selector::parse(query).map_err(|_| ())?;
	let elements = match root {
		QueryRoot::Document(doc) => doc.select(&selector).0,
		QueryRoot::Element(element) => {
			if include_root {
				element
					.select(&selector)
					.unwrap_or_else(|| HtmlElementList(Vec::new()))
					.0
			} else {
				element.select_descendants(&selector).0
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
			out
		}
		QueryRoot::Scope(scope) => scope
			.iter()
			.filter(|element| element_matches(element, &selector))
			.cloned()
			.collect(),
	};
	Ok(elements)
}

enum ExtensionFilter {
	Contains { needle: String, own: bool, negate: bool },
	Matches { pattern: Regex, own: bool, negate: bool },
	Has { selectors: Vec<String>, negate: bool },
	AttrWildcard { negate: bool },
	AttrPrefix { prefix: String, negate: bool },
	Index { kind: IndexKind, n: usize, negate: bool },
}

#[derive(Clone, Copy)]
enum IndexKind {
	Lt,
	Gt,
	Eq,
}

struct ExtensionSplit {
	prefix: String,
	filter: ExtensionFilter,
	suffix: String,
}

#[derive(Clone, Copy)]
enum MarkerKind {
	Contains { own: bool, negate: bool },
	Matches { own: bool, negate: bool },
	Has { negate: bool },
	Index { kind: IndexKind, negate: bool },
	AttrWildcard { negate: bool },
	AttrPrefix { negate: bool },
}

enum MarkerOutcome {
	Split(ExtensionSplit),
	/// 不是标记（或 :has 内容无需拦截），从给定位置继续扫描
	Skip(usize),
	Invalid,
}

/// 切出第一个 scraper 不支持的伪类标记，引号/括号/属性/转义感知（引号内的
/// `:has(` 之类不算标记）。:has/:not(:has) 内容不含扩展伪类时跳过整组不拦截；
/// 找到标记但括号不闭合时返回 None，让外层走 Selector::parse 报无效查询
fn split_extension(query: &str) -> Option<ExtensionSplit> {
	const MARKERS: &[(&str, MarkerKind)] = &[
		(":not(:containsOwn(", MarkerKind::Contains { own: true, negate: true }),
		(":not(:contains(", MarkerKind::Contains { own: false, negate: true }),
		(":not(:matchesOwn(", MarkerKind::Matches { own: true, negate: true }),
		(":not(:matches(", MarkerKind::Matches { own: false, negate: true }),
		(":not(:has(", MarkerKind::Has { negate: true }),
		(":not(:lt(", MarkerKind::Index { kind: IndexKind::Lt, negate: true }),
		(":not(:gt(", MarkerKind::Index { kind: IndexKind::Gt, negate: true }),
		(":not(:eq(", MarkerKind::Index { kind: IndexKind::Eq, negate: true }),
		(":not([^", MarkerKind::AttrPrefix { negate: true }),
		(":not([*])", MarkerKind::AttrWildcard { negate: true }),
		(":containsOwn(", MarkerKind::Contains { own: true, negate: false }),
		(":contains(", MarkerKind::Contains { own: false, negate: false }),
		(":matchesOwn(", MarkerKind::Matches { own: true, negate: false }),
		(":matches(", MarkerKind::Matches { own: false, negate: false }),
		(":has(", MarkerKind::Has { negate: false }),
		(":lt(", MarkerKind::Index { kind: IndexKind::Lt, negate: false }),
		(":gt(", MarkerKind::Index { kind: IndexKind::Gt, negate: false }),
		(":eq(", MarkerKind::Index { kind: IndexKind::Eq, negate: false }),
		("[^", MarkerKind::AttrPrefix { negate: false }),
		("[*]", MarkerKind::AttrWildcard { negate: false }),
	];
	let mut quote: Option<char> = None;
	let mut paren = 0u8;
	let mut bracket = 0u8;
	let mut i = 0;
	while i < query.len() {
		let c = query[i..].chars().next()?;
		if quote.is_none() && paren == 0 && bracket == 0
			&& let Some((marker, kind)) = MARKERS.iter().find(|(m, _)| query[i..].starts_with(m))
		{
			match parse_extension_at(query, i, marker, kind) {
				MarkerOutcome::Split(split) => return Some(split),
				MarkerOutcome::Skip(end) => {
					i = end;
					continue;
				}
				MarkerOutcome::Invalid => return None,
			}
		}
		match quote {
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == '(' => paren += 1,
			None if c == '[' => bracket += 1,
			None if c == ')' => paren = paren.saturating_sub(1),
			None if c == ']' => bracket = bracket.saturating_sub(1),
			None => {}
		}
		i += c.len_utf8();
		if c == '\\' && i < query.len() {
			// 转义的下一个字符不参与引号/括号/标记判定
			i += query[i..].chars().next().map_or(0, char::len_utf8);
		}
	}
	None
}

fn parse_extension_at(query: &str, index: usize, marker: &str, kind: &MarkerKind) -> MarkerOutcome {
	let prefix = query[..index].to_string();
	if let MarkerKind::AttrPrefix { negate } = kind {
		let content_start = index + marker.len();
		let Some(close) = find_bracket_close(query, content_start) else {
			return MarkerOutcome::Invalid;
		};
		let content = query[content_start..close].trim();
		let after = close + 1;
		let suffix = if *negate {
			// :not([^前缀]) 要求右括号收尾，否则 [:not( 里包的是别的东西
			if query[after..].starts_with(')') {
				&query[after + 1..]
			} else {
				return MarkerOutcome::Skip(index + 1);
			}
		} else {
			&query[after..]
		};
		if content.is_empty() {
			return MarkerOutcome::Invalid;
		}
		return MarkerOutcome::Split(ExtensionSplit {
			prefix,
			filter: ExtensionFilter::AttrPrefix { prefix: content.to_string(), negate: *negate },
			suffix: suffix.to_string(),
		});
	}
	if let MarkerKind::AttrWildcard { negate } = kind {
		return MarkerOutcome::Split(ExtensionSplit {
			prefix,
			filter: ExtensionFilter::AttrWildcard { negate: *negate },
			suffix: query[index + marker.len()..].to_string(),
		});
	}
	let extra_depth = u8::from(matches!(
		kind,
		MarkerKind::Contains { negate: true, .. }
			| MarkerKind::Matches { negate: true, .. }
			| MarkerKind::Has { negate: true }
			| MarkerKind::Index { negate: true, .. }
	));
	let Some((content, suffix)) = take_paren_content(query, index + marker.len(), extra_depth) else {
		return MarkerOutcome::Invalid;
	};
	match kind {
		MarkerKind::Contains { own, negate } => MarkerOutcome::Split(ExtensionSplit {
			prefix,
			filter: ExtensionFilter::Contains {
				needle: unescape_needle(&unquote(content.trim())),
				own: *own,
				negate: *negate,
			},
			suffix,
		}),
		MarkerKind::Matches { own, negate } => match Regex::new(content.trim()) {
			Ok(pattern) => MarkerOutcome::Split(ExtensionSplit {
				prefix,
				filter: ExtensionFilter::Matches { pattern, own: *own, negate: *negate },
				suffix,
			}),
			Err(_) => MarkerOutcome::Invalid,
		},
		MarkerKind::Has { negate } => {
			if split_extension(&content).is_none() {
				return MarkerOutcome::Skip(query.len() - suffix.len());
			}
			let selectors = split_top_level_commas(&content);
			if selectors.iter().any(|s| s.is_empty()) {
				return MarkerOutcome::Invalid;
			}
			MarkerOutcome::Split(ExtensionSplit {
				prefix,
				filter: ExtensionFilter::Has { selectors, negate: *negate },
				suffix,
			})
		}
		MarkerKind::Index { kind, negate } => match content.trim().parse::<usize>() {
			Ok(n) => MarkerOutcome::Split(ExtensionSplit {
				prefix,
				filter: ExtensionFilter::Index { kind: *kind, n, negate: *negate },
				suffix,
			}),
			Err(_) => MarkerOutcome::Invalid,
		},
		MarkerKind::AttrWildcard { .. } | MarkerKind::AttrPrefix { .. } => unreachable!(),
	}
}

/// 在首个顶层组合器（空白/>/+/~）处切开，引号/括号/属性/转义感知。
/// 返回（首段复合, 组合器起的剩余）
fn split_head(query: &str) -> (&str, &str) {
	let mut quote: Option<char> = None;
	let mut paren = 0u8;
	let mut bracket = 0u8;
	let mut chars = query.char_indices().peekable();
	while let Some((i, c)) = chars.next() {
		match quote {
			Some(_) if c == '\\' => {
				chars.next();
			}
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\\' => {
				chars.next();
			}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == '(' => paren += 1,
			None if c == '[' => bracket += 1,
			None if c == ')' => paren = paren.saturating_sub(1),
			None if c == ']' => bracket = bracket.saturating_sub(1),
			None if paren == 0 && bracket == 0 && (matches!(c, '>' | '+' | '~') || c.is_whitespace()) => {
				return (&query[..i], &query[i..]);
			}
			None => {}
		}
	}
	(query, "")
}

/// 从 start 扫描到配平后的右括号（extra_depth 抵消 :not 多包的一层），引号感知；
/// 反斜杠转义的字符不参与配平（`:contains(a\))` 的 \) 不算闭合）。
/// 返回（内容, 剩余后缀）；extra_depth>0 时内容裁到内层闭合处
fn take_paren_content(query: &str, start: usize, extra_depth: u8) -> Option<(String, String)> {
	let rest = &query[start..];
	let mut quote: Option<char> = None;
	let mut depth = extra_depth;
	let mut inner_end = None;
	let mut chars = rest.char_indices().peekable();
	while let Some((i, c)) = chars.next() {
		match quote {
			Some(_) if c == '\\' => {
				chars.next();
			}
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\\' => {
				chars.next();
			}
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

/// 属性选择器 `[^前缀]` 的右括号位置，引号/转义感知
fn find_bracket_close(query: &str, start: usize) -> Option<usize> {
	let rest = &query[start..];
	let mut quote: Option<char> = None;
	let mut chars = rest.char_indices().peekable();
	while let Some((i, c)) = chars.next() {
		match quote {
			Some(_) if c == '\\' => {
				chars.next();
			}
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\\' => {
				chars.next();
			}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == ']' => return Some(start + i),
			None => {}
		}
	}
	None
}

fn split_top_level_commas(text: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut quote: Option<char> = None;
	let mut paren = 0u8;
	let mut bracket = 0u8;
	let mut start = 0;
	let mut chars = text.char_indices().peekable();
	while let Some((i, c)) = chars.next() {
		match quote {
			Some(_) if c == '\\' => {
				chars.next();
			}
			Some(q) if c == q => quote = None,
			Some(_) => {}
			None if c == '\\' => {
				chars.next();
			}
			None if c == '\'' || c == '"' => quote = Some(c),
			None if c == '(' => paren += 1,
			None if c == '[' => bracket += 1,
			None if c == ')' => paren = paren.saturating_sub(1),
			None if c == ']' => bracket = bracket.saturating_sub(1),
			None if c == ',' && paren == 0 && bracket == 0 => {
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

/// jsoup TokenQueue.unescape：反斜杠只转义紧跟的一个字符
fn unescape_needle(raw: &str) -> String {
	let mut out = String::with_capacity(raw.len());
	let mut chars = raw.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some(escaped) => out.push(escaped),
				None => out.push('\\'),
			}
		} else {
			out.push(c);
		}
	}
	out
}

fn matches_extension(element: &HtmlElement, filter: &ExtensionFilter) -> Result<bool, ()> {
	Ok(match filter {
		ExtensionFilter::Contains { needle, own, negate } => {
			contains_text(element, needle, *own) != *negate
		}
		ExtensionFilter::Matches { pattern, own, negate } => {
			// jsoup :matches 针对归一化后的 text()/ownText()、find 语义、默认大小写敏感
			let text = if *own { element.own_text() } else { element.text(true) };
			text.is_some_and(|text| pattern.is_match(&text)) != *negate
		}
		ExtensionFilter::AttrWildcard { negate } => element.has_any_attribute() != *negate,
		ExtensionFilter::AttrPrefix { prefix, negate } => {
			has_attr_prefix(element, prefix) != *negate
		}
		ExtensionFilter::Index { kind, n, negate } => {
			let index = element_sibling_index(element);
			let hit = match kind {
				IndexKind::Lt => index < *n,
				IndexKind::Gt => index > *n,
				IndexKind::Eq => index == *n,
			};
			hit != *negate
		}
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

/// SwiftSoup :contains 是子树归一化文本、大小写不敏感、空白折叠后匹配
fn contains_text(element: &HtmlElement, needle: &str, own: bool) -> bool {
	let text = if own {
		element.own_text()
	} else {
		element.text(true)
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

/// SwiftSoup blockTags：text() 的分隔只对块级元素与 <br> 生效，未列出的
/// 标签（含自定义标签）按行内处理
const BLOCK_TAGS: &[&str] = &[
	"html", "head", "body", "frameset", "script", "noscript", "style", "meta", "link", "title",
	"frame", "noframes", "noembed", "section", "nav", "aside", "hgroup", "header", "footer", "p",
	"h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "pre", "listing", "div", "blockquote", "hr",
	"address", "figure", "figcaption", "form", "fieldset", "center", "dir", "applet", "marquee",
	"ins", "del", "s", "dl", "dt", "dd", "li", "table", "caption", "thead", "tfoot", "tbody",
	"colgroup", "col", "tr", "th", "td", "video", "audio", "canvas", "details", "menu", "plaintext",
	"template", "article", "main", "svg", "math",
];

/// SwiftSoup preserveWhitespaceTags：子树内文本逐字保留不折叠
const PRESERVE_WHITESPACE_TAGS: &[&str] = &["pre", "plaintext", "title", "textarea"];

/// jsoup StringUtil.isActuallyWhitespace：\xA0 算空白、U+3000 不算，
/// 比 char::is_whitespace 窄，CJK 文本里的表意空格必须保住
fn is_text_whitespace(c: char) -> bool {
	matches!(c, ' ' | '\t' | '\n' | '\x0C' | '\r' | '\u{A0}')
}

/// jsoup StringUtil.isInvisibleChar：零宽空格与软连字符不进 text()
fn is_invisible_char(c: char) -> bool {
	matches!(c, '\u{200B}' | '\u{AD}')
}

/// java String.trim() 语义：只去 <= U+0020 的字符，不是 Unicode 空白
fn jsoup_trim(text: &str) -> &str {
	text.trim_matches(|c: char| c <= '\u{20}')
}

/// jsoup Element.preserveWhitespace：父元素起向上最多 6 层查 pre 系标签
/// （ego-tree 的 ancestors 从父节点开始，不含自身）
fn preserve_whitespace(node: ego_tree::NodeRef<'_, Node>) -> bool {
	node.ancestors()
		.take(6)
		.filter_map(|ancestor| ancestor.value().as_element())
		.any(|element| PRESERVE_WHITESPACE_TAGS.contains(&element.name()))
}

/// jsoup Element.appendNormalisedText：preserveWhitespace 子树逐字追加，
/// 否则折叠连续空白为单空格；accum 为空或末尾已是空格时剥掉前导空白
fn append_normalised_text(accum: &mut String, node: ego_tree::NodeRef<'_, Node>, text: &str) {
	if preserve_whitespace(node) {
		accum.push_str(text);
		return;
	}
	let strip_leading = accum.is_empty() || accum.ends_with(' ');
	let mut last_was_white = false;
	let mut reached_non_white = false;
	for c in text.chars() {
		if is_text_whitespace(c) {
			if (strip_leading && !reached_non_white) || last_was_white {
				continue;
			}
			accum.push(' ');
			last_was_white = true;
		} else if !is_invisible_char(c) {
			accum.push(c);
			last_was_white = false;
			reached_non_white = true;
		}
	}
}

/// SwiftSoup text() 遍历：前序走子树，块级/br 元素头部在末尾非空格时补一个
/// 空格（SwiftSoup 无 jsoup 的 tail 分隔，<div>a</div>b 得 "ab"）；script/style
/// 的内容在 jsoup 是 DataNode 不进 text，html5ever 存成文本节点，靠跳过整个
/// 子树排除；normalize=false 时文本节点逐字追加（aidoku untrimmed_text）
fn collect_text(element: ElementRef<'_>, normalize: bool) -> String {
	let mut accum = String::new();
	let mut stack = vec![*element];
	while let Some(node) = stack.pop() {
		match node.value() {
			Node::Text(text) => {
				if normalize {
					append_normalised_text(&mut accum, node, text.deref());
				} else {
					accum.push_str(text.deref());
				}
				continue;
			}
			Node::Element(element) => {
				if !accum.is_empty()
					&& (BLOCK_TAGS.contains(&element.name()) || element.name() == "br")
					&& !accum.ends_with(' ')
				{
					accum.push(' ');
				}
				if matches!(element.name(), "script" | "style") {
					continue;
				}
			}
			_ => {}
		}
		stack.extend(node.children().rev());
	}
	accum
}

fn element_matches(element: &HtmlElement, selector: &Selector) -> bool {
	let html = element.0.html.borrow();
	let Some(node) = html.tree.get(element.0.id) else {
		return false;
	};
	let Some(element_ref) = ElementRef::wrap(node) else {
		return false;
	};
	selector.matches(&element_ref)
}

fn element_children(element: &HtmlElement) -> Vec<HtmlElement> {
	element.children().map(|list| list.0).unwrap_or_default()
}

/// CSS 兄弟组合器只在元素之间成立，文本/注释节点要跳过
fn next_element_sibling(element: &HtmlElement) -> Option<HtmlElement> {
	let html = element.0.html.borrow();
	let node = html.tree.get(element.0.id)?;
	let sibling = node.next_siblings().find_map(ElementRef::wrap)?;
	Some(element.child(sibling.id()))
}

fn following_element_siblings(element: &HtmlElement) -> Vec<HtmlElement> {
	let html = element.0.html.borrow();
	let Some(node) = html.tree.get(element.0.id) else {
		return Vec::new();
	};
	node.next_siblings()
		.filter_map(ElementRef::wrap)
		.map(|sibling| element.child(sibling.id()))
		.collect()
}

fn has_attr_prefix(element: &HtmlElement, prefix: &str) -> bool {
	let html = element.0.html.borrow();
	let Some(node) = html.tree.get(element.0.id) else {
		return false;
	};
	let Some(element_ref) = ElementRef::wrap(node) else {
		return false;
	};
	element_ref.value().attrs().any(|(k, _)| k.starts_with(prefix))
}

/// jsoup elementSiblingIndex：同级元素中的 0 起下标，非元素节点不计
fn element_sibling_index(element: &HtmlElement) -> usize {
	let html = element.0.html.borrow();
	let Some(node) = html.tree.get(element.0.id) else {
		return usize::MAX;
	};
	node.prev_siblings()
		.filter(|sibling| sibling.value().is_element())
		.count()
}

fn dedup_elements(elements: &mut Vec<HtmlElement>) {
	let mut seen = std::collections::HashSet::new();
	elements.retain(|element| seen.insert(element.0.id));
}

/// jsoup 逗号列表语义：并集按文档序、去重
fn sort_dedup_document_order(elements: &mut Vec<HtmlElement>) {
	let Some(html) = elements.first().map(|element| element.0.html.clone()) else {
		return;
	};
	let html = html.borrow();
	let mut order = std::collections::HashMap::new();
	for (index, node) in html.tree.root().descendants().enumerate() {
		order.insert(node.id(), index);
	}
	elements.sort_by_key(|element| order.get(&element.0.id).copied().unwrap_or(usize::MAX));
	elements.dedup_by_key(|element| element.0.id);
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
		assert!(select_str(QueryRoot::Document(&doc), "p:contains(x) +").is_err());
	}

	#[test]
	fn contains_sibling_continuations() {
		let html = r#"<!DOCTYPE html><html><body>
		<ul>
		<li>狀態： <span class="txt">連載中</span></li>
		<li>作者： <span class="txt">某人</span></li>
		<li>其他</li>
		</ul>
		<p>x</p>
		<p>first</p>
		<span>gap</span>
		<p>second</p>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		// + 紧邻兄弟（跳过文本节点）
		let list = select_str(QueryRoot::Document(&doc), "li:contains(狀態：) + li").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("作者： 某人"));

		// ~ 后续全部兄弟
		let list = select_str(QueryRoot::Document(&doc), "li:contains(狀態：) ~ li").unwrap();
		assert_eq!(list.0.len(), 2);

		let list = select_str(QueryRoot::Document(&doc), "p:contains(x) + p").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("first"));

		// + 只取紧邻的一个，中间隔了 span 的不再命中
		let list = select_str(QueryRoot::Document(&doc), "p:contains(x) ~ p").unwrap();
		assert_eq!(list.0.len(), 2);
	}

	#[test]
	fn contains_adjacent_sibling_source_shape() {
		// zh.relamanhua 的真实写法（issue #83 的原始报告）
		let html = r#"<!DOCTYPE html><html><body>
		<div class="comicParticulars-synopsis">
		<span class="comicParticulars-left-txt">作者：</span>
		<span class="comicParticulars-right-txt"><a href="/author/1">某人</a></span>
		<span class="comicParticulars-left-txt">題材：</span>
		<span class="comicParticulars-right-txt"><a href="/genre/1">熱血</a></span>
		<span class="comicParticulars-left-txt">狀態：</span>
		<span class="comicParticulars-right-txt"><a href="/status/1">連載中</a></span>
		</div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		for (label, href) in [
			("作者：", "/author/1"),
			("題材：", "/genre/1"),
			("狀態：", "/status/1"),
		] {
			let query = format!("span:contains({label}) + span.comicParticulars-right-txt a");
			let list = select_str(QueryRoot::Document(&doc), &query).unwrap();
			assert_eq!(list.0.len(), 1, "{query}");
			assert_eq!(list.0[0].attr("href").as_deref(), Some(href), "{query}");
		}
	}

	#[test]
	fn has_sibling_continuation() {
		let html = r#"<!DOCTYPE html><html><body>
		<div><a href="/1">x</a></div>
		<p>p1</p>
		<p>p2</p>
		<div>plain</div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let list = select_str(QueryRoot::Document(&doc), "div:has(a) ~ p").unwrap();
		assert_eq!(list.0.len(), 2);
		assert_eq!(list.0[0].text(true).as_deref(), Some("p1"));
	}

	#[test]
	fn has_relative_selector_passthrough() {
		let html = r#"<!DOCTYPE html><html><body>
		<div id="a"><a href="/1">x</a></div>
		<div id="b"><p><a href="/2">y</a></p></div>
		<div id="c"><p>z</p></div>
		<div id="d"></div><p>after</p>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		// :has/:not 内容不含扩展伪类时整个交给 Selector::parse（CSS4 相对选择器）
		let list = select_str(QueryRoot::Document(&doc), "div:has(> a)").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("a"));

		let list = select_str(QueryRoot::Document(&doc), "div:not(:has(> a))").unwrap();
		assert_eq!(list.0.len(), 3);

		let list = select_str(QueryRoot::Document(&doc), "div:has(+ p)").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("d"));

		// 内容含扩展伪类时仍走拦截路径
		let list = select_str(QueryRoot::Document(&doc), "div:has(p:contains(z))").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("c"));
	}

	#[test]
	fn same_compound_continuation_on_candidate() {
		let html = r#"<!DOCTYPE html><html><body>
		<ul>
		<li class="cls keep" data-k="v">x one</li>
		<li class="keep">x two</li>
		<li class="cls">y three <span class="cls">nested</span></li>
		</ul>
		<div id="ab"><a href="/1">x</a><b>y</b></div>
		<div id="a"><a href="/2">x</a></div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		// 同复合剩余必须作用于候选元素自身，不是后代
		let list = select_str(QueryRoot::Document(&doc), "li:contains(x).cls").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("x one"));

		let list = select_str(QueryRoot::Document(&doc), "li:contains(x)[data-k]").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("x one"));

		let list = select_str(QueryRoot::Document(&doc), "li:contains(x):not(.cls)").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("x two"));

		let list = select_str(QueryRoot::Document(&doc), "li:contains(x):contains(two)").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("x two"));

		let list = select_str(QueryRoot::Document(&doc), "div:has(a):has(b)").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("ab"));
	}

	#[test]
	fn child_combinator_after_extension() {
		let html = r#"<!DOCTYPE html><html><body>
		<ul><li>x<span class="inner">a<span class="inner">b</span></span></li></ul>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		// > 是直接子元素，不是后代：嵌套的第二个 span.inner 不能命中
		let list = select_str(QueryRoot::Document(&doc), "li:contains(x) > span.inner").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].children().unwrap().0.len(), 1);
	}

	#[test]
	fn index_selectors() {
		let html = r#"<!DOCTYPE html><html><body>
		<table><tr><td>c0</td><td>c1</td><td>c2</td><td>c3</td></tr></table>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let texts = |query: &str| -> Vec<String> {
			select_str(QueryRoot::Document(&doc), query)
				.unwrap()
				.0
				.iter()
				.map(|e| e.text(true).unwrap())
				.collect()
		};

		// jsoup :lt/:gt/:eq 是同级元素下标（0 起）
		assert_eq!(texts("td:lt(2)"), ["c0", "c1"]);
		assert_eq!(texts("td:gt(1)"), ["c2", "c3"]);
		assert_eq!(texts("td:eq(2)"), ["c2"]);
		assert_eq!(texts("td:eq(0)"), ["c0"]);
		assert_eq!(texts("td:not(:eq(0))"), ["c1", "c2", "c3"]);
	}

	#[test]
	fn matches_regex_selectors() {
		let html = r#"<!DOCTYPE html><html><body>
		<p id="p1">Chapter 12</p>
		<p id="p2">no digits</p>
		<p id="p3">sub <b>3</b> own</p>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);
		let ids = |query: &str| -> Vec<String> {
			select_str(QueryRoot::Document(&doc), query)
				.unwrap()
				.0
				.iter()
				.map(|e| e.id().unwrap())
				.collect()
		};

		assert_eq!(ids(r"p:matches(\d+)"), ["p1", "p3"]);
		// own 文本：p3 的数字在子元素里
		assert_eq!(ids(r"p:matchesOwn(\d+)"), ["p1"]);
		// 默认大小写敏感，(?i) 显式放开（jsoup 同款）
		assert_eq!(ids(r"p:matches(chapter)"), Vec::<String>::new());
		assert_eq!(ids(r"p:matches((?i)chapter)"), ["p1"]);
		assert!(select_str(QueryRoot::Document(&doc), r"p:matches([)").is_err());
	}

	#[test]
	fn attr_prefix_selector() {
		let html = r#"<!DOCTYPE html><html><body>
		<div id="d1" data-id="1" data-x="2">a</div>
		<div id="d2" title="t">b</div>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(QueryRoot::Document(&doc), "div[^data-]").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("d1"));

		let list = select_str(QueryRoot::Document(&doc), "div:not([^data-])").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].id().as_deref(), Some("d2"));
	}

	#[test]
	fn marker_inside_quoted_attr_value_not_split() {
		let html = r#"<!DOCTYPE html><html><body>
		<a href="/1" title=":has(">x</a>
		<a href="/2">y</a>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(QueryRoot::Document(&doc), r#"a[title=":has("]"#).unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].attr("href").as_deref(), Some("/1"));

		let list = select_str(QueryRoot::Document(&doc), r#"a[title=":has("]:contains(x)"#).unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].attr("href").as_deref(), Some("/1"));
	}

	#[test]
	fn contains_backslash_escape() {
		let html = r#"<!DOCTYPE html><html><body>
		<p>a) b</p>
		<p>other</p>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(QueryRoot::Document(&doc), r"p:contains(a\))").unwrap();
		assert_eq!(list.0.len(), 1);
		assert_eq!(list.0[0].text(true).as_deref(), Some("a) b"));
	}

	#[test]
	fn top_level_comma_union() {
		let html = r#"<!DOCTYPE html><html><body>
		<h2>has y</h2>
		<h1>has x</h1>
		<h3>plain</h3>
		</body></html>"#;
		let doc = HtmlDocument::parse(html, None);

		let list = select_str(QueryRoot::Document(&doc), "h1:contains(x), h2:contains(y)").unwrap();
		assert_eq!(list.0.len(), 2);
		// 并集按文档序（jsoup 逗号列表语义），不是分支顺序
		assert_eq!(list.0[0].tag_name().as_deref(), Some("h2"));
		assert_eq!(list.0[1].tag_name().as_deref(), Some("h1"));

		// 重复命中要去重
		let list = select_str(QueryRoot::Document(&doc), "h1:contains(x), h1").unwrap();
		assert_eq!(list.0.len(), 1);
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
		assert!(list.0[0].data().unwrap().contains("var x"));

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

	#[test]
	fn set_attr_visibility_and_cache_reset() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);
		let first_p = content.select_first(&Selector::parse("p.text").unwrap()).unwrap();

		first_p.0.set_attr("data-k", "v1").unwrap();
		assert_eq!(first_p.attr("data-k").as_deref(), Some("v1"));
		first_p.0.set_attr("data-k", "v2").unwrap();
		assert_eq!(first_p.attr("data-k").as_deref(), Some("v2"));

		// id/classes 有 OnceCell 缓存，attr 写入走 Element::new 重建后 #id 选择器必须看到新值
		first_p.0.set_attr("id", "target").unwrap();
		assert_eq!(doc.select(&Selector::parse("#target").unwrap()).0.len(), 1);

		first_p.0.remove_attr("data-k").unwrap();
		assert_eq!(first_p.attr("data-k"), None);
		first_p.0.remove_attr("data-k").unwrap();

		// 文本节点挂不了属性
		let text_node = content
			.0
			.child_nodes()
			.unwrap()
			.into_iter()
			.find(|n| n.is_text())
			.unwrap();
		assert!(text_node.set_attr("k", "v").is_none());
	}

	#[test]
	fn set_text_replaces_children() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);
		let first_p = content.select_first(&Selector::parse("p.text").unwrap()).unwrap();

		first_p.0.set_text("only <b> & more").unwrap();
		assert_eq!(first_p.text(true).as_deref(), Some("only <b> & more"));
		assert_eq!(
			first_p.outer_html().as_deref(),
			Some(r#"<p class="text">only &lt;b&gt; &amp; more</p>"#)
		);
		assert_eq!(first_p.0.child_nodes().unwrap().len(), 1);
	}

	#[test]
	fn fragment_splice_order() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);

		content.0.append_html("<em>a</em>x<b>b</b>").unwrap();
		content.0.prepend_html("<i>h</i>").unwrap();
		let inner = content.html().unwrap();
		assert!(inner.starts_with("<i>h</i>"), "got: {inner}");
		assert!(inner.ends_with("<em>a</em>x<b>b</b>"), "got: {inner}");

		// 拼进来的节点对同一文档的 select 可见
		assert_eq!(doc.select(&Selector::parse("em").unwrap()).0.len(), 1);
		assert_eq!(doc.select(&Selector::parse("div i").unwrap()).0.len(), 1);

		// set_html 先清空再拼接
		content.0.set_html("<span>only</span>").unwrap();
		assert_eq!(content.html().as_deref(), Some("<span>only</span>"));
		assert_eq!(doc.select(&Selector::parse("p.text").unwrap()).0.len(), 0);
		// 空 fragment 是合法 no-op
		content.0.append_html("").unwrap();
		assert_eq!(content.html().as_deref(), Some("<span>only</span>"));
	}

	#[test]
	fn class_ops_normalize_and_stay_visible() {
		let doc = HtmlDocument::parse(
			r#"<!DOCTYPE html><html><body><div id="d" class="x  x y">t</div></body></html>"#,
			None,
		);
		let el = doc.select(&Selector::parse("#d").unwrap()).0.into_iter().next().unwrap();

		el.0.add_class("z").unwrap();
		// 重复空白与重复 class 被归一化（jsoup classNames 集合语义）
		assert_eq!(el.class_name().as_deref(), Some("x y z"));
		el.0.add_class("z").unwrap();
		assert_eq!(el.class_name().as_deref(), Some("x y z"));
		assert_eq!(doc.select(&Selector::parse("#d.z").unwrap()).0.len(), 1);

		el.0.remove_class("x").unwrap();
		assert_eq!(el.class_name().as_deref(), Some("y z"));
		el.0.remove_class("missing").unwrap();
		assert_eq!(el.class_name().as_deref(), Some("y z"));
		// 清空后保留空 class 属性（jsoup 恒写回）
		el.0.remove_class("y").unwrap();
		el.0.remove_class("z").unwrap();
		assert!(el.has_attr("class"));
		assert_eq!(el.class_name().as_deref(), Some(""));
	}

	#[test]
	fn detach_keeps_subtree_usable() {
		let doc = HtmlDocument::parse(HTML, None);
		let content = content_element(&doc);
		let first_p = content.select_first(&Selector::parse("p.text").unwrap()).unwrap();

		first_p.0.detach().unwrap();
		assert_eq!(doc.select(&Selector::parse("p.text").unwrap()).0.len(), 1);
		// ego-tree detach 保留子树：被摘下的元素仍可读写，parent 变 None
		assert_eq!(first_p.text(true).as_deref(), Some("Hello & goodbye"));
		first_p.0.set_text("changed").unwrap();
		assert_eq!(first_p.text(true).as_deref(), Some("changed"));
		assert!(first_p.parent().is_none());
		assert!(first_p.0.parent().is_none());
	}

	fn first_element(doc: &HtmlDocument, selector: &str) -> HtmlElement {
		doc.select(&Selector::parse(selector).unwrap())
			.0
			.into_iter()
			.next()
			.unwrap()
	}

	#[test]
	fn text_block_br_and_whitespace() {
		// B1（issue #83）：块级边界补一个空格，内部空白折叠
		let doc = HtmlDocument::parse(r#"<div id="d"><p>a</p><p>b</p></div>"#, None);
		assert_eq!(first_element(&doc, "#d").text(true).as_deref(), Some("a b"));

		let doc = HtmlDocument::parse("<p>a<br>b</p>", None);
		assert_eq!(first_element(&doc, "p").text(true).as_deref(), Some("a b"));

		let doc = HtmlDocument::parse("<p>  Hello   there\n now  </p>", None);
		let p = first_element(&doc, "p");
		assert_eq!(p.text(true).as_deref(), Some("Hello there now"));
		// trimmed=false 逐字保留（aidoku untrimmed_text：不归一化不 trim）
		assert_eq!(p.text(false).as_deref(), Some("  Hello   there\n now  "));

		// aidoku-rs 文档里的 untrimmed_text 示例
		let doc = HtmlDocument::parse("<p>Hello <b>there</b> now! </p>", None);
		let p = first_element(&doc, "p");
		assert_eq!(p.text(false).as_deref(), Some("Hello there now! "));
		assert_eq!(p.text(true).as_deref(), Some("Hello there now!"));

		// untrimmed 同样补块级分隔、不含尾部 trim
		let doc = HtmlDocument::parse(r#"<div id="u">a<p>b</p></div>"#, None);
		assert_eq!(first_element(&doc, "#u").text(false).as_deref(), Some("a b"));
	}

	#[test]
	fn text_excludes_script_style_content() {
		// B2（issue #83）：script/style 内容（jsoup DataNode）不进 text；
		// 元素自身是块级分隔，jsoup 1.15.3 / SwiftSoup 2.13.7 实测均为 "x y"
		let doc = HtmlDocument::parse(
			r#"<div id="e">x<script>var s=1;</script>y</div><div id="s">x<style>.a{color:red}</style>y</div>"#,
			None,
		);
		let e = first_element(&doc, "#e");
		assert_eq!(e.text(true).as_deref(), Some("x y"));
		assert_eq!(e.text(false).as_deref(), Some("x y"));
		assert_eq!(first_element(&doc, "#s").text(true).as_deref(), Some("x y"));

		// script 元素自身 text 为空，内容只能走 data()
		let doc = HtmlDocument::parse("<script>var x = 1;</script>", None);
		let script = first_element(&doc, "script");
		assert_eq!(script.text(true).as_deref(), Some(""));
		assert_eq!(script.own_text().as_deref(), Some(""));
		assert_eq!(script.data().as_deref(), Some("var x = 1;"));

		// 脚本内容不参与 :contains/:matches 的匹配文本
		let doc = HtmlDocument::parse("<div>x<script>needle</script>y</div>", None);
		assert!(
			select_str(QueryRoot::Document(&doc), "div:contains(needle)")
				.unwrap()
				.0
				.is_empty()
		);
		assert!(
			select_str(QueryRoot::Document(&doc), "div:matchesOwn(needle)")
				.unwrap()
				.0
				.is_empty()
		);
	}

	#[test]
	fn own_text_direct_text_children() {
		// C（issue #83）：own_text 拼接直系文本节点并归一化
		let doc = HtmlDocument::parse(r#"<div id="d">A<b>B</b>C</div>"#, None);
		let d = first_element(&doc, "#d");
		assert_eq!(d.own_text().as_deref(), Some("AC"));
		assert_eq!(d.text(true).as_deref(), Some("ABC"));

		let doc = HtmlDocument::parse("<div>A  <b>B</b>  C</div>", None);
		assert_eq!(first_element(&doc, "div").own_text().as_deref(), Some("A C"));

		// 直系 <br> 也补空格
		let doc = HtmlDocument::parse("<p>a<br>b</p>", None);
		assert_eq!(first_element(&doc, "p").own_text().as_deref(), Some("a b"));

		let doc = HtmlDocument::parse(r#"<div id="c">A<b>B</b>C</div>"#, None);
		assert_eq!(
			select_str(QueryRoot::Document(&doc), "#c:containsOwn(AC)")
				.unwrap()
				.0
				.len(),
			1
		);
		assert!(
			select_str(QueryRoot::Document(&doc), "#c:containsOwn(ABC)")
				.unwrap()
				.0
				.is_empty()
		);
	}

	#[test]
	fn contains_and_matches_use_normalized_text() {
		// jsoup :contains/:matches 针对归一化后的 text()：块级边界带空格
		let doc = HtmlDocument::parse(r#"<div id="m"><p>hello</p><p>world</p></div>"#, None);
		assert_eq!(
			select_str(QueryRoot::Document(&doc), "div:contains(hello world)")
				.unwrap()
				.0
				.len(),
			1
		);
		assert!(
			select_str(QueryRoot::Document(&doc), "div:contains(helloworld)")
				.unwrap()
				.0
				.is_empty()
		);
		assert_eq!(
			select_str(QueryRoot::Document(&doc), "#m:matches(hello world)")
				.unwrap()
				.0
				.len(),
			1
		);
		assert!(
			select_str(QueryRoot::Document(&doc), "#m:matches(helloworld)")
				.unwrap()
				.0
				.is_empty()
		);
	}

	#[test]
	fn text_preserve_whitespace_and_special_chars() {
		// jsoup preserveWhitespace 标签内文本逐字保留
		let doc = HtmlDocument::parse("<pre id=\"p\">a\n  b</pre>", None);
		assert_eq!(first_element(&doc, "#p").text(true).as_deref(), Some("a\n  b"));
		let doc = HtmlDocument::parse(r#"<textarea id="t">a
  b</textarea>"#, None);
		assert_eq!(first_element(&doc, "#t").text(true).as_deref(), Some("a\n  b"));

		// jsoup 的空白集不含 U+3000（表意空格不折叠），零宽字符直接丢弃
		let doc = HtmlDocument::parse("<div>a\u{3000}b</div>", None);
		assert_eq!(first_element(&doc, "div").text(true).as_deref(), Some("a\u{3000}b"));
		let doc = HtmlDocument::parse("<p>a\u{200B}b</p>", None);
		assert_eq!(first_element(&doc, "p").text(true).as_deref(), Some("ab"));

		// nbsp 归入普通空格参与折叠
		let doc = HtmlDocument::parse("<p>a&nbsp;&nbsp;b</p>", None);
		assert_eq!(first_element(&doc, "p").text(true).as_deref(), Some("a b"));
	}

	#[test]
	fn text_no_tail_separator() {
		// SwiftSoup（Aidoku 实际依赖）没有 jsoup 的 tail 分隔：块级元素后紧跟
		// 文本时 text() 不补空格（SwiftSoup 2.13.7 实测 "ab"，jsoup 1.15.3 是 "a b"）
		let doc = HtmlDocument::parse(r#"<div id="t">a</div>b"#, None);
		assert_eq!(first_element(&doc, "body").text(true).as_deref(), Some("ab"));
	}

	// body 子节点固定为 [div#a, Text(" text "), div#b, Comment, div#c]
	const SIBLINGS_HTML: &str = r#"<!DOCTYPE html><html><body><div id="a">a</div> text <div id="b">b</div><!--x--><div id="c">c</div></body></html>"#;

	#[test]
	fn element_next_prev_sibling_skips_non_elements() {
		// D1（issue #83）：jsoup nextElementSibling/previousElementSibling
		// 沿兄弟链找第一个元素节点，文本/注释不算
		let doc = HtmlDocument::parse(SIBLINGS_HTML, None);
		let a = first_element(&doc, "#a");
		let b = first_element(&doc, "#b");
		let c = first_element(&doc, "#c");

		// a 的紧邻兄弟是文本节点，next 必须跳到 #b
		assert_eq!(a.next_sibling().unwrap().id().as_deref(), Some("b"));
		// b 的紧邻后方是注释，next 必须跳到 #c
		assert_eq!(b.next_sibling().unwrap().id().as_deref(), Some("c"));
		assert!(c.next_sibling().is_none());

		assert_eq!(c.prev_sibling().unwrap().id().as_deref(), Some("b"));
		assert_eq!(b.prev_sibling().unwrap().id().as_deref(), Some("a"));
		assert!(a.prev_sibling().is_none());
	}

	#[test]
	fn element_siblings_both_directions_elements_only() {
		// D2（issue #83）：jsoup siblingElements() 是父元素的全部元素子节点，
		// 排除自身、按文档序（双向，文本/注释不计）
		let doc = HtmlDocument::parse(SIBLINGS_HTML, None);
		let sibling_ids = |selector: &str| -> Vec<String> {
			first_element(&doc, selector)
				.siblings()
				.unwrap()
				.0
				.iter()
				.map(|element| element.id().unwrap())
				.collect()
		};
		assert_eq!(sibling_ids("#a"), ["b", "c"]);
		assert_eq!(sibling_ids("#b"), ["a", "c"]);
		assert_eq!(sibling_ids("#c"), ["a", "b"]);

		// 摘下的元素没有父节点，兄弟列表为空（jsoup 对无父元素给空列表）
		let doc = HtmlDocument::parse(SIBLINGS_HTML, None);
		let b = first_element(&doc, "#b");
		b.0.detach().unwrap();
		assert!(b.siblings().unwrap().0.is_empty());
		assert!(b.0.siblings().unwrap().is_empty());
	}

	#[test]
	fn node_siblings_both_directions_all_node_kinds() {
		// D3（issue #83）：jsoup siblingNodes() 双向全部兄弟节点、排除自身、
		// 按文档序；node 层不按元素过滤，文本/注释在内
		let doc = HtmlDocument::parse(SIBLINGS_HTML, None);
		let b = first_element(&doc, "#b");
		let siblings = b.0.siblings().unwrap();
		assert_eq!(siblings.len(), 4);
		let first = HtmlElement::try_from(siblings[0].clone()).ok().unwrap();
		assert_eq!(first.id().as_deref(), Some("a"));
		assert!(siblings[1].is_text());
		assert!(siblings[2].is_comment());
		let last = HtmlElement::try_from(siblings[3].clone()).ok().unwrap();
		assert_eq!(last.id().as_deref(), Some("c"));

		// 非元素节点自身的兄弟列表同样双向、含全部节点
		let text_node = first_element(&doc, "#a").0.next_sibling().unwrap();
		assert!(text_node.is_text());
		assert_eq!(text_node.siblings().unwrap().len(), 4);
	}

	#[test]
	fn node_next_prev_stay_adjacent_nodes() {
		// node 层 next/prev 保持紧邻节点导航、不按元素过滤（vendor 文档
		// "Get the next sibling of the node"，SwiftSoup Node.nextSibling）
		let doc = HtmlDocument::parse(SIBLINGS_HTML, None);

		let text = first_element(&doc, "#a").0.next_sibling().unwrap();
		assert!(text.is_text());
		assert_eq!(text.text().as_deref(), Some(" text "));
		assert!(text.prev_sibling().unwrap().is_element());

		let comment = first_element(&doc, "#b").0.next_sibling().unwrap();
		assert!(comment.is_comment());
		let c = comment.next_sibling().unwrap();
		assert_eq!(HtmlElement::try_from(c).ok().unwrap().id().as_deref(), Some("c"));
	}
}
