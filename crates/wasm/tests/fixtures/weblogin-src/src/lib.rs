#![no_std]
//! WebLoginHandler 导出的端到端验证源：
//! 三个 login 设置分别覆盖 host 解析的三种路径（显式 url / url_key 指向 defaults /
//! 无 url 走 base_url 兜底），clear_cookies_on_log_out 两态俱备；
//! handle_web_login 只有同时拿到 cookies 参数且从 defaults 读到预置的
//! `<key>.keys`（Aidoku 的顺序：先存后调，源靠 defaults_get_map 取 cookie）
//! 且含 session 项才判成功，观测结果写进 defaults 供宿主侧断言。
//! 断言方在 crates/wasm/tests/weblogin_host.rs 与 crates/server/tests/source_login.rs。
use aidoku::{
	Chapter, FilterValue, HashMap, LoginMethod, LoginSetting, Manga, MangaPageResult, Page, Result,
	Setting, Source, TextSetting, WebLoginHandler,
	alloc::{String, Vec, string::ToString, vec},
	imports::defaults::{DefaultValue, defaults_get, defaults_set},
	prelude::*,
};

struct WebLoginFixtureSource;

fn observed(key: &str, cookies: &HashMap<String, String>) {
	let mut pairs: Vec<String> = cookies
		.iter()
		.map(|(name, value)| {
			let mut pair = name.to_string();
			pair.push('=');
			pair.push_str(value);
			pair
		})
		.collect();
	pairs.sort();
	let keys: Vec<String> = defaults_get(&format!("{key}.keys")).unwrap_or_default();
	defaults_set(
		"weblogin.seen",
		DefaultValue::StringArray(vec![key.to_string(), pairs.join(","), keys.join(",")]),
	);
}

impl Source for WebLoginFixtureSource {
	fn new() -> Self {
		Self
	}

	/// 把 login marker 与 keys 编进标题：重启后的实例读到的 defaults 与库一致才算存活
	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let marker: String = defaults_get("login").unwrap_or_default();
		let keys: Vec<String> = defaults_get("login.keys").unwrap_or_default();
		Ok(MangaPageResult {
			entries: vec![Manga {
				key: "state".into(),
				title: format!("marker={marker};keys={}", keys.join(",")),
				..Manga::default()
			}],
			has_next_page: false,
		})
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl aidoku::DynamicSettings for WebLoginFixtureSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			LoginSetting {
				key: "login".into(),
				title: "登录".into(),
				method: LoginMethod::Web,
				url: Some("https://login.example.com/signin".into()),
				logout_title: Some("退出登录".into()),
				local_storage_keys: Some(vec!["token".into()]),
				clear_cookies_on_log_out: true,
				..LoginSetting::default()
			}
			.into(),
			LoginSetting {
				key: "login_keep".into(),
				title: "保 cookie 登录".into(),
				method: LoginMethod::Web,
				url_key: Some("auth_url".into()),
				..LoginSetting::default()
			}
			.into(),
			LoginSetting {
				key: "login_fallback".into(),
				title: "兜底登录".into(),
				method: LoginMethod::Web,
				clear_cookies_on_log_out: true,
				..LoginSetting::default()
			}
			.into(),
			TextSetting {
				key: "auth_url".into(),
				title: "认证地址".into(),
				default: Some("https://auth.example.org/".into()),
				..TextSetting::default()
			}
			.into(),
		])
	}
}

impl WebLoginHandler for WebLoginFixtureSource {
	fn handle_web_login(&self, key: String, cookies: HashMap<String, String>) -> Result<bool> {
		observed(&key, &cookies);
		// 空 cookies 是登出语义：源确认即可，不需要任何凭据
		if cookies.is_empty() {
			return Ok(true);
		}
		let keys: Vec<String> = defaults_get(&format!("{key}.keys")).unwrap_or_default();
		Ok(cookies.contains_key("session") && keys.iter().any(|k| k == "session"))
	}
}

register_source!(WebLoginFixtureSource, DynamicSettings, WebLoginHandler);
