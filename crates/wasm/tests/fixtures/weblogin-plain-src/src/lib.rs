#![no_std]
//! 不导出 handle_web_login 的对照源：声明同款 web 登录设置，
//! 服务端对这种源只存 cookie 与 marker 不调导出（issue #69 的验收分支）。
//! 断言方在 crates/server/tests/source_login.rs。
use aidoku::{
	Chapter, FilterValue, LoginMethod, LoginSetting, Manga, MangaPageResult, Page, Result, Setting,
	Source, TextSetting,
	alloc::{String, Vec, vec},
	prelude::*,
};

struct WebLoginPlainFixtureSource;

impl Source for WebLoginPlainFixtureSource {
	fn new() -> Self {
		Self
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		Ok(MangaPageResult {
			entries: Vec::new(),
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

impl aidoku::DynamicSettings for WebLoginPlainFixtureSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		Ok(vec![
			LoginSetting {
				key: "login".into(),
				title: "登录".into(),
				method: LoginMethod::Web,
				url: Some("https://login.example.com/signin".into()),
				local_storage_keys: Some(vec!["token".into()]),
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

register_source!(WebLoginPlainFixtureSource, DynamicSettings);
