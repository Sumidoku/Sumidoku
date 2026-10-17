use wasmer::Module;

#[derive(Debug, Clone, Default)]
pub struct SourceFeatures {
	pub manga_list: bool,
	pub home: bool,
	pub dynamic_listings: bool,
	pub dynamic_filters: bool,
	pub dynamic_settings: bool,
	pub page_image_processor: bool,
	pub cover_image_processor: bool,
	pub image_request: bool,
	pub page_description: bool,
	pub alternate_covers: bool,
	pub base_url: bool,
	pub notification: bool,
	pub deep_link: bool,
	pub basic_login: bool,
	pub web_login: bool,
	pub migration: bool,
}

impl SourceFeatures {
	pub fn from_module(module: &Module) -> Self {
		let has = |name: &str| module.exports().any(|e| e.name() == name);
		Self {
			manga_list: has("get_manga_list"),
			home: has("get_home"),
			dynamic_listings: has("get_listings"),
			dynamic_filters: has("get_filters"),
			dynamic_settings: has("get_settings"),
			page_image_processor: has("process_page_image"),
			cover_image_processor: has("process_cover_image"),
			image_request: has("get_image_request"),
			page_description: has("get_page_description"),
			alternate_covers: has("get_alternate_covers"),
			base_url: has("get_base_url"),
			notification: has("handle_notification"),
			deep_link: has("handle_deep_link"),
			basic_login: has("handle_basic_login"),
			web_login: has("handle_web_login"),
			migration: has("handle_key_migration"),
		}
	}
}
