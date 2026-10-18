#![no_std]
//! NotificationHandler 导出的端到端验证源：
//! handle_notification 把收到的字符串记进实例状态，get_dynamic_settings 把它
//! 编进 echo 设置的标题，宿主侧经 GET /sources/{id}/settings 读回断言送达。
//! 静态 settings.json 与动态项的合并去重也靠它（动态键 notifyOnly、与静态
//! 撞车的 dup）。断言方在 crates/server/tests/source_settings.rs。
use aidoku::{
	Chapter, FilterValue, Manga, MangaPageResult, NotificationHandler, Page, Result, Setting, Source,
	TextSetting, ToggleSetting,
	alloc::{String, Vec, vec},
	prelude::*,
};
use core::cell::RefCell;

struct NotifyFixtureSource {
	seen: RefCell<Option<String>>,
}

impl Source for NotifyFixtureSource {
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
		bail!("not used")
	}

	fn get_manga_update(&self, manga: Manga, _needs_details: bool, _needs_chapters: bool) -> Result<Manga> {
		Ok(manga)
	}

	fn get_page_list(&self, _manga: Manga, _chapter: Chapter) -> Result<Vec<Page>> {
		Ok(Vec::new())
	}
}

impl aidoku::DynamicSettings for NotifyFixtureSource {
	fn get_dynamic_settings(&self) -> Result<Vec<Setting>> {
		let seen = self.seen.borrow().clone().unwrap_or_else(|| "none".into());
		Ok(vec![
			TextSetting {
				key: "echo".into(),
				title: seen.into(),
				notification: Some("echo-changed".into()),
				..TextSetting::default()
			}
			.into(),
			ToggleSetting {
				key: "notifyOnly".into(),
				title: "仅动态".into(),
				..ToggleSetting::default()
			}
			.into(),
			ToggleSetting {
				key: "dup".into(),
				title: "动态 dup".into(),
				..ToggleSetting::default()
			}
			.into(),
		])
	}
}

impl NotificationHandler for NotifyFixtureSource {
	fn handle_notification(&self, notification: String) {
		*self.seen.borrow_mut() = Some(notification);
	}
}

register_source!(NotifyFixtureSource, DynamicSettings, NotificationHandler);
