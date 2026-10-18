use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Default)]
pub struct NetConfig {
	pub direct_domains: Vec<String>,
	pub relay_domains: Vec<String>,
	pub trawl_proxy: Option<String>,
	pub trawl_ca: Option<PathBuf>,
	pub proxy: Option<String>,
	pub allow_direct_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
	pub addr: String,
	pub config_dir: PathBuf,
	pub database_url: String,
	pub call_concurrency: usize,
	pub log: String,
	pub covers_dir: PathBuf,
	pub cover_cache_max: usize,
	pub webui_dir: PathBuf,
	pub user_agent: String,
	pub downloads_dir: PathBuf,
	pub download_concurrency: usize,
	pub download_format: String,
	pub update_interval_hours: u64,
	pub auto_download_new_chapters: bool,
	pub auto_download_limit: usize,
	pub opds_lazy_download: bool,
	pub basic_password: Option<String>,
	pub net: NetConfig,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			addr: "0.0.0.0:3000".into(),
			config_dir: PathBuf::from("./data"),
			database_url: "sqlite://data/sumidoku.db?mode=rwc".into(),
			call_concurrency: 8,
			log: "info".into(),
			covers_dir: PathBuf::from("./data/covers"),
			cover_cache_max: 512,
			webui_dir: PathBuf::from("./webui/build"),
			user_agent: DEFAULT_USER_AGENT.into(),
			downloads_dir: PathBuf::from("./downloads"),
			download_concurrency: 3,
			download_format: "cbz".into(),
			update_interval_hours: 0,
			auto_download_new_chapters: false,
			auto_download_limit: 5,
			opds_lazy_download: false,
			basic_password: None,
			net: NetConfig::default(),
		}
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct NetConfigFile {
	direct_domains: Option<Vec<String>>,
	relay_domains: Option<Vec<String>>,
	trawl_proxy: Option<String>,
	trawl_ca: Option<PathBuf>,
	proxy: Option<String>,
	allow_direct_fallback: Option<bool>,
}

impl Config {
	/// db、covers、sources 全部钉在 config_dir 下派生，数据根只有这一个配置项
	fn derive_paths(&mut self) {
		self.database_url = format!("sqlite://{}/sumidoku.db?mode=rwc", self.config_dir.display());
		self.covers_dir = self.config_dir.join("covers");
	}

	/// 源包目录钉死在 config_dir 下，不开放配置
	pub fn sources_dir(&self) -> PathBuf {
		self.config_dir.join("sources")
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
	addr: Option<String>,
	config_dir: Option<PathBuf>,
	call_concurrency: Option<usize>,
	log: Option<String>,
	cover_cache_max: Option<usize>,
	webui_dir: Option<PathBuf>,
	user_agent: Option<String>,
	downloads_dir: Option<PathBuf>,
	download_concurrency: Option<usize>,
	download_format: Option<String>,
	update_interval_hours: Option<u64>,
	auto_download_new_chapters: Option<bool>,
	auto_download_limit: Option<usize>,
	opds_lazy_download: Option<bool>,
	basic_password: Option<String>,
	net: Option<NetConfigFile>,
}

impl Config {
	/// 默认值 ← ./config.toml（可缺省）← 环境变量（SUMIDOKU_ 前缀）
	pub fn load() -> Self {
		let mut config = Self::default();
		if let Ok(raw) = std::fs::read_to_string("./config.toml") {
			match toml::from_str::<ConfigFile>(&raw) {
				Ok(file) => config.apply_file(file),
				Err(e) => eprintln!("invalid ./config.toml, using defaults: {e}"),
			}
		}
		config.apply_env();
		config.derive_paths();
		config
	}

	fn apply_file(&mut self, file: ConfigFile) {
		if let Some(v) = file.addr {
			self.addr = v;
		}
		if let Some(v) = file.config_dir {
			self.config_dir = v;
		}
		if let Some(v) = file.call_concurrency {
			self.call_concurrency = v;
		}
		if let Some(v) = file.log {
			self.log = v;
		}
		if let Some(v) = file.cover_cache_max {
			self.cover_cache_max = v;
		}
		if let Some(v) = file.webui_dir {
			self.webui_dir = v;
		}
		if let Some(v) = file.user_agent {
			self.user_agent = v;
		}
		if let Some(v) = file.downloads_dir {
			self.downloads_dir = v;
		}
		if let Some(v) = file.download_concurrency {
			self.download_concurrency = v;
		}
		if let Some(v) = file.download_format {
			self.download_format = v;
		}
		if let Some(v) = file.update_interval_hours {
			self.update_interval_hours = v;
		}
		if let Some(v) = file.auto_download_new_chapters {
			self.auto_download_new_chapters = v;
		}
		if let Some(v) = file.auto_download_limit {
			self.auto_download_limit = v;
		}
		if let Some(v) = file.opds_lazy_download {
			self.opds_lazy_download = v;
		}
		if let Some(v) = file.basic_password {
			self.basic_password = Some(v);
		}
		if let Some(net) = file.net {
			if let Some(v) = net.direct_domains {
				self.net.direct_domains = v;
			}
			if let Some(v) = net.relay_domains {
				self.net.relay_domains = v;
			}
			if net.trawl_proxy.is_some() {
				self.net.trawl_proxy = net.trawl_proxy;
			}
			if net.trawl_ca.is_some() {
				self.net.trawl_ca = net.trawl_ca;
			}
			if net.proxy.is_some() {
				self.net.proxy = net.proxy;
			}
			if let Some(v) = net.allow_direct_fallback {
				self.net.allow_direct_fallback = v;
			}
		}
	}

	fn apply_env(&mut self) {
		if let Ok(v) = std::env::var("SUMIDOKU_ADDR") {
			self.addr = v;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_CONFIG_DIR") {
			self.config_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_CALL_CONCURRENCY")
			&& let Ok(n) = v.parse()
		{
			self.call_concurrency = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_LOG") {
			self.log = v;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_COVER_CACHE_MAX")
			&& let Ok(n) = v.parse()
		{
			self.cover_cache_max = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_WEBUI_DIR") {
			self.webui_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_USER_AGENT") {
			self.user_agent = v;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_DOWNLOADS_DIR") {
			self.downloads_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_DOWNLOAD_CONCURRENCY")
			&& let Ok(n) = v.parse()
		{
			self.download_concurrency = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_DOWNLOAD_FORMAT") {
			self.download_format = v;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_UPDATE_INTERVAL_HOURS")
			&& let Ok(n) = v.parse()
		{
			self.update_interval_hours = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_AUTO_DOWNLOAD_NEW_CHAPTERS")
			&& let Ok(n) = v.parse()
		{
			self.auto_download_new_chapters = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_AUTO_DOWNLOAD_LIMIT")
			&& let Ok(n) = v.parse()
		{
			self.auto_download_limit = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_OPDS_LAZY_DOWNLOAD")
			&& let Ok(n) = v.parse()
		{
			self.opds_lazy_download = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BASIC_PASSWORD") {
			self.basic_password = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_DIRECT_DOMAINS") {
			self.net.direct_domains = split_csv(&v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_RELAY_DOMAINS") {
			self.net.relay_domains = split_csv(&v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_TRAWL_PROXY") {
			self.net.trawl_proxy = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_TRAWL_CA") {
			self.net.trawl_ca = Some(PathBuf::from(v));
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_PROXY") {
			self.net.proxy = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_ALLOW_DIRECT_FALLBACK")
			&& let Ok(n) = v.parse()
		{
			self.net.allow_direct_fallback = n;
		}
	}
}

fn split_csv(raw: &str) -> Vec<String> {
	raw.split(',')
		.map(|s| s.trim().to_string())
		.filter(|s| !s.is_empty())
		.collect()
}
