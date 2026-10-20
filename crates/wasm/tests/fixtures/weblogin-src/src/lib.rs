#![no_std]
//! End-to-end verification source for the WebLoginHandler export:
//! the three login settings cover the three paths the host resolves (explicit url /
//! url_key pointing into defaults / no url falling back to base_url), with both states of
//! clear_cookies_on_log_out present; handle_web_login judges success only when it both
//! receives the cookies parameter and reads the preset `<key>.keys` from defaults
//! (Aidoku's order: store first, then call — the source reads cookies via
//! defaults_get_map) containing a session entry; observations are written into defaults
//! for the host side to assert.
//! The asserting sides are crates/wasm/tests/weblogin_host.rs and crates/server/tests/source_login.rs.
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

	/// Weave the login marker and keys into the title: a restarted instance only counts as surviving if the defaults it reads match the store
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
		// Empty cookies are the log-out semantic: the source just acknowledges them, no credentials needed
		if cookies.is_empty() {
			return Ok(true);
		}
		let keys: Vec<String> = defaults_get(&format!("{key}.keys")).unwrap_or_default();
		Ok(cookies.contains_key("session") && keys.iter().any(|k| k == "session"))
	}
}

register_source!(WebLoginFixtureSource, DynamicSettings, WebLoginHandler);
