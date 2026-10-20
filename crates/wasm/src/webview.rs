//! Server-side sandbox modeling of WebView: the server has no browser, so ABI functions with
//! pure script-execution semantics land on JsEngine — one webview gets one independent engine
//! instance, shared with no js context (mirroring iOS AidokuRunner's WebViewHandler /
//! IsolatedJSContext split).
//!
//! Trade-offs of page modeling (there is no DOM; document cannot be provided honestly, so we
//! do not fake a half-broken API):
//! - Only two browser globals are installed: window = globalThis and location.href = base URL
//!   (about:blank when there is no base URL, matching loadHTMLString's empty-base behavior);
//!   the window.x / location.href that page scripts touch most therefore work, while touching
//!   document.* throws and lands in the log
//! - load_html matches loadHTMLString's document-replacement semantics: the engine is rebuilt
//!   (old globals vanish with the old document) and inline <script>s run in document order;
//!   each script succeeds or fails on its own — a single throw only goes to the log and
//!   affects neither the load result nor later scripts (same as a browser)
//! - Scripts with src are skipped: the sandbox has no document loader and fetches no network
//!   resources; a non-JavaScript type (e.g. application/json) is not executed per browser
//!   rules; type=module runs as classic — a bare Context has no module evaluation entry, so
//!   module-specific syntax errors into the log
//! - User scripts wrap page scripts by injection timing (at_document_start/end);
//!   for_main_frame_only is ignored: the sandbox has no frame concept
//! - Rebuilding the engine on timeout is existing JsEngine behavior; page state (including
//!   the two shims above) is dropped with it, same contract as context_* dropping global
//!   state on timeout

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

	// Returns the lines for the source log: script errors and skipped external scripts
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
		// The shim source is a fixed template; failing to install it can only mean the engine itself is broken — leave it for later eval calls to report
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

// Aligned with browser script-execution rules: only a missing/empty type or a JavaScript
// MIME executes; module runs as classic (see the module header); importmap/JSON and other
// data blocks do not execute
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
