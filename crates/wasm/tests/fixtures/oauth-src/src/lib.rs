#![no_std]
//! OAuth 登录（method: "oauth"）的端到端验证源：
//! - login_oauth：pkce + token_url 的交换分支，授权地址内嵌 client_id/redirect_uri
//!   供交换时捕获；token_url 从 defaults 的 oauth_token_url 读——mock 令牌端点的
//!   端口编译期不可知，动态设置在运行时按当前 defaults 拼
//! - login_oauth_plain：无 token_url 的回调串分支，notification 送达串编进 echo 标题
//! - login_oauth_key：url_key 指向 oauth_auth_url 的默认值，兜底解析授权地址
//! get_search_manga_list 把 login_oauth 的值编进标题，重启存活断言靠它。
//! 断言方在 crates/server/tests/source_login_oauth.rs。
use aidoku::{
	Chapter, FilterValue, LoginMethod, LoginSetting, Manga, MangaPageResult, NotificationHandler, Page, Result,
	Setting, Source, TextSetting,
	alloc::{String, Vec, vec},
	imports::defaults::defaults_get,
	prelude::*,
};
use core::cell::RefCell;

struct OAuthFixtureSource {
	seen: RefCell<Option<String>>,
}

impl Source for OAuthFixtureSource {
	fn new() -> Self {
		Self {
			seen: RefCell::new(None),
		}
	}

	fn get_search_manga_list(
		&self,
		_query: Option<String>,
		_page: i32,
		_filters: Vec<FilterValue>,
	) -> Result<MangaPageResult> {
		let token: String = defaults_get("login_oauth").unwrap_or_default();
		Ok(MangaPageResult {
			entries: vec![Manga {
				key: "state".into(),
				title: format!("token={token}"),
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

impl aidoku::DynamicSettings for OAuthFixtureSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let token_url: String = defaults_get("oauth_token_url").unwrap_or_default();
		let seen = self.seen.borrow().clone().unwrap_or_else(|| "none".into());
		Ok(vec![
			LoginSetting {
				key: "login_oauth".into(),
				title: "OAuth 登录".into(),
				method: LoginMethod::OAuth,
				url: Some(
					"https://auth.example.com/authorize?client_id=c1&redirect_uri=oauthdemo%3A%2F%2Fcb".into(),
				),
				logout_title: Some("退出 OAuth".into()),
				pkce: true,
				token_url: (!token_url.is_empty()).then(|| token_url.into()),
				callback_scheme: Some("oauthdemo".into()),
				..LoginSetting::default()
			}
			.into(),
			LoginSetting {
				key: "login_oauth_plain".into(),
				title: "回调串登录".into(),
				method: LoginMethod::OAuth,
				url: Some("https://auth.example.com/plain".into()),
				notification: Some("oauth-logged-in".into()),
				..LoginSetting::default()
			}
			.into(),
			LoginSetting {
				key: "login_oauth_key".into(),
				title: "url_key 登录".into(),
				method: LoginMethod::OAuth,
				url_key: Some("oauth_auth_url".into()),
				pkce: true,
				..LoginSetting::default()
			}
			.into(),
			TextSetting {
				key: "oauth_auth_url".into(),
				title: "授权地址".into(),
				default: Some(
					"https://auth.example.org/authorize?client_id=c2&redirect_uri=https%3A%2F%2Fapp.example.io%2Fcb"
						.into(),
				),
				..TextSetting::default()
			}
			.into(),
			TextSetting {
				key: "oauth_token_url".into(),
				title: "令牌端点".into(),
				..TextSetting::default()
			}
			.into(),
			TextSetting {
				key: "echo".into(),
				title: seen.into(),
				..TextSetting::default()
			}
			.into(),
		])
	}
}

impl NotificationHandler for OAuthFixtureSource {
	fn handle_notification(&self, notification: String) {
		*self.seen.borrow_mut() = Some(notification);
	}
}

register_source!(OAuthFixtureSource, DynamicSettings, NotificationHandler);
