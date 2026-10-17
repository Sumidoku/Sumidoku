//! 宿主侧解码回归测试：源侧定制 Serialize 与配对 Deserialize 的 postcard roundtrip。
#![cfg(feature = "imports")]

use aidoku::*;
use std::{borrow::Cow, string::String, vec, vec::Vec};

fn roundtrip<T>(value: &T)
where
	T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + core::fmt::Debug,
{
	let bytes = postcard::to_allocvec(value).expect("serialize");
	let decoded: T = postcard::from_bytes(&bytes).expect("deserialize");
	assert_eq!(&decoded, value);
}

#[test]
fn filter_all_kinds() {
	let filters = vec![
		TextFilter {
			id: "text".into(),
			title: Some("Text".into()),
			placeholder: Some("Search".into()),
			..Default::default()
		}
		.into(),
		SortFilter {
			id: "sort".into(),
			title: Some("Sort".into()),
			can_ascend: true,
			options: vec!["Popular".into(), "Recent".into()],
			default: Some(SortFilterDefault {
				index: 1,
				ascending: false,
			}),
			..Default::default()
		}
		.into(),
		CheckFilter {
			id: "check".into(),
			title: Some("Check".into()),
			can_exclude: true,
			..Default::default()
		}
		.into(),
		SelectFilter {
			id: "select".into(),
			title: Some("Select".into()),
			uses_tag_style: true,
			options: vec!["One".into(), "Two".into()],
			..Default::default()
		}
		.into(),
		MultiSelectFilter {
			id: "mselect".into(),
			title: Some("Multi".into()),
			can_exclude: true,
			options: vec!["One".into(), "Two".into()],
			default_included: Some(vec!["One".into()]),
			..Default::default()
		}
		.into(),
		Filter::note("a note"),
		RangeFilter {
			id: "range".into(),
			title: Some("Range".into()),
			min: Some(0.0),
			max: Some(100.0),
			decimal: true,
			..Default::default()
		}
		.into(),
	];
	roundtrip(&filters);
}

#[test]
fn settings_roundtrip() {
	let settings: Vec<Setting> = vec![
		ToggleSetting {
			key: "toggle".into(),
			title: "Toggle".into(),
			default: true,
			..Default::default()
		}
		.into(),
		Setting {
			key: Cow::Borrowed("login"),
			title: Cow::Borrowed("Login"),
			notification: None,
			requires: None,
			requires_false: None,
			refreshes: None,
			value: SettingValue::Login {
				method: LoginMethod::OAuth,
				url: Some(Cow::Borrowed("https://example.com")),
				url_key: None,
				logout_title: None,
				pkce: true,
				token_url: None,
				callback_scheme: None,
				use_email: false,
				local_storage_keys: None,
				clear_cookies_on_log_out: false,
			},
		},
		Setting {
			key: Cow::Borrowed("page"),
			title: Cow::Borrowed("Page"),
			notification: None,
			requires: None,
			requires_false: None,
			refreshes: Some(vec![Cow::Borrowed("settings")]),
			value: SettingValue::Page {
				items: vec![],
				inline_title: Some(true),
				auth_to_open: None,
				icon: Some(PageIcon::System {
					name: String::from("gear"),
					color: String::from("#fff"),
					inset: Some(2),
				}),
				info: None,
			},
		},
		Setting {
			key: Cow::Borrowed("icon-url"),
			title: Cow::Borrowed("IconUrl"),
			notification: None,
			requires: None,
			requires_false: None,
			refreshes: None,
			value: SettingValue::Page {
				items: vec![],
				inline_title: None,
				auth_to_open: None,
				icon: Some(PageIcon::Url(String::from("https://example.com/i.png"))),
				info: None,
			},
		},
	];
	roundtrip(&settings);
}

#[test]
fn manga_page_result() {
	let result = MangaPageResult {
		entries: vec![Manga {
			key: String::from("1"),
			title: String::from("Manga 1"),
			..Default::default()
		}],
		has_next_page: true,
	};
	roundtrip(&result);
}

#[test]
fn deep_link_results() {
	roundtrip(&DeepLinkResult::Manga {
		key: String::from("m"),
	});
	roundtrip(&DeepLinkResult::Chapter {
		manga_key: String::from("m"),
		key: String::from("c"),
	});
	roundtrip(&DeepLinkResult::Listing(Listing {
		id: String::from("l"),
		name: String::from("L"),
		kind: ListingKind::List,
	}));
}

#[test]
fn page_contents() {
	let pages = vec![
		Page {
			content: PageContent::url("https://example.com/1.jpg"),
			has_description: true,
			description: Some(String::from("desc")),
			..Default::default()
		},
		Page {
			content: PageContent::text("# Title"),
			..Default::default()
		},
		Page {
			content: PageContent::Zip(String::from("https://example.com/a.zip"), String::from("1.jpg")),
			..Default::default()
		},
	];
	roundtrip(&pages);
}
