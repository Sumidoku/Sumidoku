//! chromiumoxide 实装的 CDP client。异步 API 收进每源 actor 线程持有的
//! current_thread runtime：block_on 驱动命令与后台 Handler 同生共死，
//! 宿主调用保持同步签名，与 reqwest::blocking 的 net 栈同一线程模型。
use super::{BrowserCookie, BrowserError, CdpClient};
use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::network::DeleteCookiesParams;
use chromiumoxide::cdp::browser_protocol::storage::{ClearCookiesParams, GetCookiesParams};
use chromiumoxide::cdp::browser_protocol::target::CreateTargetParams;
use chromiumoxide::error::CdpError;
use chromiumoxide::page::Page;
use futures::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

pub struct ChromiumoxideClient {
	runtime: tokio::runtime::Runtime,
	browser: Browser,
	pages: HashMap<String, Page>,
}

impl ChromiumoxideClient {
	pub fn connect(ws: &str) -> Result<Self, BrowserError> {
		let runtime = tokio::runtime::Builder::new_current_thread()
			.enable_all()
			.build()
			.map_err(|e| BrowserError::fatal(format!("browser runtime: {e}")))?;
		let (browser, mut handler) = runtime
			.block_on(Browser::connect(ws))
			.map_err(|e| BrowserError::fatal(format!("browser connect failed: {e}")))?;
		// Handler 驱动 websocket 收发；只在 block_on 窗口内被轮询，
		// 对本模块这种纯请求/响应式用法足够
		runtime.spawn(async move { while handler.next().await.is_some() {} });
		Ok(Self { runtime, browser, pages: HashMap::new() })
	}

	fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
		self.runtime.block_on(future)
	}

	fn page(&mut self, context: &str) -> Result<&Page, BrowserError> {
		if !self.pages.contains_key(context) {
			let params = CreateTargetParams {
				url: "about:blank".into(),
				browser_context_id: Some(BrowserContextId::from(context.to_string())),
				..CreateTargetParams::default()
			};
			let page = self
				.block_on(self.browser.new_page(params))
				.map_err(|e| classify("browser page create failed", e))?;
			self.pages.insert(context.to_string(), page);
		}
		Ok(self.pages.get(context).expect("page just created"))
	}
}

/// 连接死亡（websocket/通道）才 fatal；CDP 报错与超时是单次失败
fn classify(op: &str, e: CdpError) -> BrowserError {
	let fatal = matches!(e, CdpError::Ws(_) | CdpError::ChannelSendError(_) | CdpError::NoResponse);
	BrowserError { message: format!("{op}: {e}"), fatal }
}

impl From<chromiumoxide::cdp::browser_protocol::network::Cookie> for BrowserCookie {
	fn from(cookie: chromiumoxide::cdp::browser_protocol::network::Cookie) -> Self {
		// CDP 会话 cookie 的 expires 是 -1
		let expires = (cookie.expires > 0.0).then_some(cookie.expires as i64);
		Self {
			name: cookie.name,
			value: cookie.value,
			domain: cookie.domain,
			path: cookie.path,
			expires,
			secure: cookie.secure,
			http_only: cookie.http_only,
		}
	}
}

impl CdpClient for ChromiumoxideClient {
	fn create_context(&mut self) -> Result<String, BrowserError> {
		let params = chromiumoxide::cdp::browser_protocol::target::CreateBrowserContextParams::default();
		let id = self
			.block_on(self.browser.create_browser_context(params))
			.map_err(|e| classify("browser context create failed", e))?;
		Ok(id.as_ref().to_string())
	}

	fn dispose_context(&mut self, context: &str) -> Result<(), BrowserError> {
		self.pages.remove(context);
		self.block_on(
			self.browser
				.dispose_browser_context(BrowserContextId::from(context.to_string())),
		)
		.map_err(|e| classify("browser context dispose failed", e))
	}

	fn load(
		&mut self,
		context: &str,
		url: &str,
		headers: &[(String, String)],
		timeout: Duration,
	) -> Result<Vec<BrowserCookie>, BrowserError> {
		let page = self.page(context)?.clone();
		if !headers.is_empty() {
			let map: serde_json::Map<String, serde_json::Value> = headers
				.iter()
				.map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
				.collect();
			let params = chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams::new(
				chromiumoxide::cdp::browser_protocol::network::Headers::new(serde_json::Value::Object(map)),
			);
			self.block_on(page.execute(params))
				.map_err(|e| classify("browser set headers failed", e))?;
		}
		// goto 在页面 load 事件后返回，与 iOS WKWebView didFinish 同语义
		let navigation = self.block_on(async { tokio::time::timeout(timeout, page.goto(url)).await });
		match navigation {
			Ok(Ok(_)) => {}
			Ok(Err(e)) => return Err(classify("browser navigation failed", e)),
			Err(_) => return Err(BrowserError::transient("browser navigation timed out")),
		}
		self.cookies(context)
	}

	fn cookies(&mut self, context: &str) -> Result<Vec<BrowserCookie>, BrowserError> {
		let params = GetCookiesParams {
			browser_context_id: Some(BrowserContextId::from(context.to_string())),
		};
		let cookies = self
			.block_on(self.browser.execute(params))
			.map_err(|e| classify("browser cookies read failed", e))?
			.result
			.cookies;
		Ok(cookies.into_iter().map(BrowserCookie::from).collect())
	}

	fn delete_cookie(
		&mut self,
		context: &str,
		name: &str,
		domain: Option<&str>,
		path: Option<&str>,
	) -> Result<(), BrowserError> {
		let page = self.page(context)?.clone();
		let params = DeleteCookiesParams {
			domain: domain.map(str::to_string),
			path: path.map(str::to_string),
			..DeleteCookiesParams::new(name)
		};
		// Network 域命令走 page session，天然落在该 page 的 context 上
		self.block_on(page.delete_cookies(vec![params]))
			.map_err(|e| classify("browser cookie delete failed", e))?;
		Ok(())
	}

	fn clear_cookies(&mut self, context: &str) -> Result<(), BrowserError> {
		let params = ClearCookiesParams {
			browser_context_id: Some(BrowserContextId::from(context.to_string())),
		};
		self.block_on(self.browser.execute(params))
			.map_err(|e| classify("browser cookies clear failed", e))?;
		Ok(())
	}
}
