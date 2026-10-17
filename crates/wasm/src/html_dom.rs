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
}
