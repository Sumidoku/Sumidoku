//! WebView 的服务器端沙箱建模：服务器没有浏览器，ABI 里纯脚本运行语义的函数
//! 落在 JsEngine 上——一个 webview 一个独立引擎实例，与 js context 互不共享
//! （对齐 iOS AidokuRunner 的 WebViewHandler / IsolatedJSContext 分离）。
//!
//! 页面建模的取舍（没有 DOM，document 无法诚实提供，不造假半残 API）：
//! - 只装两个浏览器全局：window = globalThis、location.href = base URL（无
//!   base URL 时 about:blank，对齐 loadHTMLString 的空基址行为）；页面脚本最
//!   常碰的 window.x / location.href 因此可用，碰 document.* 的会抛错进日志
//! - load_html 对齐 loadHTMLString 的 document 替换语义：重建引擎（旧全局随
//!   旧 document 消失），按文档序执行内联 <script>；每个脚本独立成败，单个
//!   抛错只进日志，不影响加载结果与后续脚本（浏览器同此行为）
//! - 带 src 的脚本跳过：沙箱没有 document loader 不去抓网络资源；type 非
//!   JavaScript（如 application/json）按浏览器规则不执行；type=module 按
//!   classic 跑——裸 Context 没有 module 求值入口，模块特有语法会报错进日志
//! - user script 按注入时机包在页面脚本前后（at_document_start/end），
//!   for_main_frame_only 忽略：沙箱没有 frame 概念
//! - 超时重建引擎是 JsEngine 的既有行为，页面状态（含上面两个 shim）随之
//!   丢弃，与 context_* 超时丢全局状态同一契约

use crate::js_engine::JsEngine;
use scraper::{Html, Selector};
use std::time::Duration;

pub struct WebView {
	engine: JsEngine,
	user_scripts_start: Vec<String>,
	user_scripts_end: Vec<String>,
}

impl WebView {
	pub fn new(timeout: Duration, loop_max: u64) -> Self {
		let mut view = Self {
			engine: JsEngine::new(timeout, loop_max),
			user_scripts_start: Vec::new(),
			user_scripts_end: Vec::new(),
		};
		view.install_shim(None);
		view
	}

	pub fn add_user_script(&mut self, source: String, at_document_end: bool) {
		if at_document_end {
			self.user_scripts_end.push(source);
		} else {
			self.user_scripts_start.push(source);
		}
	}

	// 返回要进源日志的行：脚本错误与被跳过的外链脚本
	pub fn load_html(&mut self, html: &str, base_url: Option<&str>) -> Vec<String> {
		self.engine.reset();
		self.install_shim(base_url);
		let mut logs = Vec::new();
		let scripts = extract_scripts(html, &mut logs);
		let start = self.user_scripts_start.iter().map(|s| ("user script", s));
		let page = scripts.iter().map(|s| ("script", s));
		let end = self.user_scripts_end.iter().map(|s| ("user script", s));
		for (kind, source) in start.chain(page).chain(end) {
			if let Err(msg) = self.engine.eval(source) {
				logs.push(format!("webview {kind} failed: {}", truncate(&msg)));
			}
		}
		logs
	}

	pub fn eval(&mut self, source: &str) -> Result<String, String> {
		self.engine.eval(source)
	}

	pub fn eval_async(&mut self, source: &str) -> Result<String, String> {
		self.engine.eval_async(source)
	}

	fn install_shim(&mut self, base_url: Option<&str>) {
		let href = serde_json::to_string(base_url.unwrap_or("about:blank")).expect("str 序列化不会失败");
		// shim 源码是固定模板，装不上只可能是引擎本身已坏，留给后续 eval 报错
		let _ = self.engine.eval(&format!("var window = globalThis; var location = {{ href: {href} }};"));
	}
}

fn extract_scripts(html: &str, logs: &mut Vec<String>) -> Vec<String> {
	let document = Html::parse_document(html);
	let selector = Selector::parse("script").expect("静态选择器不会失败");
	let mut scripts = Vec::new();
	let mut external = 0;
	for element in document.select(&selector) {
		if element.value().attr("src").is_some() {
			external += 1;
			continue;
		}
		if !is_javascript_type(element.value().attr("type")) {
			continue;
		}
		scripts.push(element.text().collect::<String>());
	}
	if external > 0 {
		logs.push(format!(
			"webview_load_html: skipped {external} external script(s), sandbox has no document loader"
		));
	}
	scripts
}

// 与浏览器的脚本执行规则对齐：缺省/空 type 与 JavaScript MIME 才执行，
// module 按 classic 跑（见模块头），importmap/JSON 等数据块不执行
fn is_javascript_type(ty: Option<&str>) -> bool {
	let Some(ty) = ty else { return true };
	let ty = ty.trim();
	if ty.is_empty() {
		return true;
	}
	let ty = ty.to_ascii_lowercase();
	ty == "module" || ty.ends_with("javascript") || ty.ends_with("ecmascript")
}

fn truncate(msg: &str) -> String {
	msg.chars().take(200).collect()
}
