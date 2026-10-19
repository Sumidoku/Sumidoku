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
	/// CDP websocket 地址（ws://…），未配置时 webview 浏览器函数显式不支持
	pub browser_ws: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
	/// 全站密码（web UI 登录、/api/v1、OPDS 共用）；None = 认证关闭
	pub password: Option<String>,
	/// 长期 API key，Bearer 直用；配置面明文、不落库
	pub api_keys: Vec<String>,
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
	/// 源仓库 index 定时刷新周期（小时）；0 = 关闭，只靠手动刷新
	pub repo_refresh_interval_hours: u64,
	pub auto_download_new_chapters: bool,
	pub auto_download_limit: usize,
	pub opds_lazy_download: bool,
	/// 源实例空闲逐出阈值（分钟）；<= 0 关闭清扫任务
	pub source_idle_evict_minutes: i64,
	pub wasm_memory_max_mb: u64,
	pub wasm_fuel: u64,
	pub wasm_call_timeout_secs: u64,
	/// 以下四项是 setting 表 backup.* 的 env 覆盖（headless 用）；Some 时压过 setting 里的值
	pub backup_enabled: Option<bool>,
	pub backup_interval_hours: Option<u64>,
	pub backup_retention_count: Option<u32>,
	/// 备份落点，允许在 config_dir 外
	pub backup_dir: Option<PathBuf>,
	/// legacy：auth.password 的兼容别名，只在 auth.password 缺席时生效
	pub basic_password: Option<String>,
	pub auth: AuthConfig,
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
			repo_refresh_interval_hours: 1,
			auto_download_new_chapters: false,
			auto_download_limit: 5,
			opds_lazy_download: false,
			source_idle_evict_minutes: 30,
			wasm_memory_max_mb: 128,
			wasm_fuel: 10_000_000_000,
			wasm_call_timeout_secs: 60,
			backup_enabled: None,
			backup_interval_hours: None,
			backup_retention_count: None,
			backup_dir: None,
			basic_password: None,
			auth: AuthConfig::default(),
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
	browser_ws: Option<String>,
}

impl Config {
	/// db、covers、sources 全部钉在 config_dir 下派生，数据根只有这一个配置项
	fn derive_paths(&mut self) {
		self.database_url = format!("sqlite://{}/sumidoku.db?mode=rwc", self.config_dir.display());
		self.covers_dir = self.config_dir.join("covers");
	}

	/// auth.password 优先；缺席时 legacy basic_password 顶上（OPDS 时代的旧配置继续生效）
	pub fn effective_password(&self) -> Option<&str> {
		self.auth.password.as_deref().or(self.basic_password.as_deref())
	}

	/// 源包目录钉死在 config_dir 下，不开放配置
	pub fn sources_dir(&self) -> PathBuf {
		self.config_dir.join("sources")
	}

	/// PageContent::Image 物化出的页图 blob 目录（<chapter_id>/<idx>.png），同样钉死
	pub fn page_blobs_dir(&self) -> PathBuf {
		self.config_dir.join("page-blobs")
	}

	/// 定时备份的默认落点；backup_dir（env）或 setting 表 backup.dir 都可改
	pub fn default_backups_dir(&self) -> PathBuf {
		self.config_dir.join("backups")
	}

	/// 组装透传给 wasm 运行时的实例上限；sleep/net/js/browser 各档不开放配置
	pub fn wasm_limits(&self) -> aidoku_wasm::Limits {
		aidoku_wasm::Limits {
			memory_max: self.wasm_memory_max_mb * 1024 * 1024,
			fuel: self.wasm_fuel,
			call_timeout: std::time::Duration::from_secs(self.wasm_call_timeout_secs),
			..aidoku_wasm::Limits::default()
		}
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AuthConfigFile {
	password: Option<String>,
	api_keys: Option<Vec<String>>,
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
	repo_refresh_interval_hours: Option<u64>,
	auto_download_new_chapters: Option<bool>,
	auto_download_limit: Option<usize>,
	opds_lazy_download: Option<bool>,
	source_idle_evict_minutes: Option<i64>,
	wasm_memory_max_mb: Option<u64>,
	wasm_fuel: Option<u64>,
	wasm_call_timeout_secs: Option<u64>,
	backup_enabled: Option<bool>,
	backup_interval_hours: Option<u64>,
	backup_retention_count: Option<u32>,
	backup_dir: Option<PathBuf>,
	basic_password: Option<String>,
	auth: Option<AuthConfigFile>,
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
		if let Some(v) = file.repo_refresh_interval_hours {
			self.repo_refresh_interval_hours = v;
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
		if let Some(v) = file.source_idle_evict_minutes {
			self.source_idle_evict_minutes = v;
		}
		if let Some(v) = file.wasm_memory_max_mb {
			self.wasm_memory_max_mb = v;
		}
		if let Some(v) = file.wasm_fuel {
			self.wasm_fuel = v;
		}
		if let Some(v) = file.wasm_call_timeout_secs {
			self.wasm_call_timeout_secs = v;
		}
		if file.backup_enabled.is_some() {
			self.backup_enabled = file.backup_enabled;
		}
		if file.backup_interval_hours.is_some() {
			self.backup_interval_hours = file.backup_interval_hours;
		}
		if file.backup_retention_count.is_some() {
			self.backup_retention_count = file.backup_retention_count;
		}
		if file.backup_dir.is_some() {
			self.backup_dir = file.backup_dir;
		}
		if let Some(v) = file.basic_password {
			self.basic_password = Some(v);
		}
		if let Some(auth) = file.auth {
			if auth.password.is_some() {
				self.auth.password = auth.password;
			}
			if let Some(v) = auth.api_keys {
				self.auth.api_keys = v;
			}
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
			if net.browser_ws.is_some() {
				self.net.browser_ws = net.browser_ws;
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
		if let Ok(v) = std::env::var("SUMIDOKU_REPO_REFRESH_INTERVAL_HOURS")
			&& let Ok(n) = v.parse()
		{
			self.repo_refresh_interval_hours = n;
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
		if let Ok(v) = std::env::var("SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES")
			&& let Ok(n) = v.parse()
		{
			self.source_idle_evict_minutes = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_WASM_MEMORY_MAX_MB")
			&& let Ok(n) = v.parse()
		{
			self.wasm_memory_max_mb = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_WASM_FUEL")
			&& let Ok(n) = v.parse()
		{
			self.wasm_fuel = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_WASM_CALL_TIMEOUT_SECS")
			&& let Ok(n) = v.parse()
		{
			self.wasm_call_timeout_secs = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BACKUP_ENABLED")
			&& let Ok(n) = v.parse()
		{
			self.backup_enabled = Some(n);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BACKUP_INTERVAL_HOURS")
			&& let Ok(n) = v.parse()
		{
			self.backup_interval_hours = Some(n);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BACKUP_RETENTION_COUNT")
			&& let Ok(n) = v.parse()
		{
			self.backup_retention_count = Some(n);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BACKUP_DIR") {
			self.backup_dir = Some(PathBuf::from(v));
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BASIC_PASSWORD") {
			self.basic_password = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_AUTH_PASSWORD") {
			self.auth.password = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_AUTH_API_KEYS") {
			self.auth.api_keys = split_csv(&v);
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
		if let Ok(v) = std::env::var("SUMIDOKU_NET_BROWSER_WS") {
			self.net.browser_ws = Some(v);
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn browser_ws_from_file() {
		let mut config = Config::default();
		assert_eq!(config.net.browser_ws, None);
		let file: ConfigFile = toml::from_str("[net]\nbrowser_ws = \"ws://browserless:3000\"\n").expect("toml");
		config.apply_file(file);
		assert_eq!(config.net.browser_ws.as_deref(), Some("ws://browserless:3000"));
	}

	#[test]
	fn browser_ws_from_env() {
		// nextest 每测试一个进程，改环境变量不串扰
		unsafe { std::env::set_var("SUMIDOKU_NET_BROWSER_WS", "ws://127.0.0.1:3000") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_NET_BROWSER_WS") };
		assert_eq!(config.net.browser_ws.as_deref(), Some("ws://127.0.0.1:3000"));
	}

	#[test]
	fn idle_evict_and_wasm_limits_from_file() {
		let file: ConfigFile = toml::from_str(
			"source_idle_evict_minutes = 10\nwasm_memory_max_mb = 64\nwasm_fuel = 500\nwasm_call_timeout_secs = 5\n",
		)
		.expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.source_idle_evict_minutes, 10);
		assert_eq!(config.wasm_memory_max_mb, 64);
		assert_eq!(config.wasm_fuel, 500);
		assert_eq!(config.wasm_call_timeout_secs, 5);
	}

	#[test]
	fn idle_evict_and_wasm_limits_from_env() {
		unsafe {
			std::env::set_var("SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES", "-1");
			std::env::set_var("SUMIDOKU_WASM_MEMORY_MAX_MB", "96");
			std::env::set_var("SUMIDOKU_WASM_FUEL", "999");
			std::env::set_var("SUMIDOKU_WASM_CALL_TIMEOUT_SECS", "15");
		}
		let mut config = Config::default();
		config.apply_env();
		unsafe {
			std::env::remove_var("SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES");
			std::env::remove_var("SUMIDOKU_WASM_MEMORY_MAX_MB");
			std::env::remove_var("SUMIDOKU_WASM_FUEL");
			std::env::remove_var("SUMIDOKU_WASM_CALL_TIMEOUT_SECS");
		}
		assert_eq!(config.source_idle_evict_minutes, -1);
		assert_eq!(config.wasm_memory_max_mb, 96);
		assert_eq!(config.wasm_fuel, 999);
		assert_eq!(config.wasm_call_timeout_secs, 15);
	}

	#[test]
	fn repo_refresh_interval_from_file_and_env() {
		let file: ConfigFile = toml::from_str("repo_refresh_interval_hours = 12\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.repo_refresh_interval_hours, 12);

		unsafe { std::env::set_var("SUMIDOKU_REPO_REFRESH_INTERVAL_HOURS", "6") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_REPO_REFRESH_INTERVAL_HOURS") };
		assert_eq!(config.repo_refresh_interval_hours, 6);
	}

	#[test]
	fn wasm_limits_defaults() {
		let limits = Config::default().wasm_limits();
		assert_eq!(limits.memory_max, 128 * 1024 * 1024);
		assert_eq!(limits.fuel, 10_000_000_000);
		assert_eq!(limits.call_timeout, std::time::Duration::from_secs(60));
	}

	#[test]
	fn wasm_limits_mapping() {
		let config = Config {
			wasm_memory_max_mb: 64,
			wasm_fuel: 123,
			wasm_call_timeout_secs: 7,
			..Config::default()
		};
		let limits = config.wasm_limits();
		assert_eq!(limits.memory_max, 64 * 1024 * 1024);
		assert_eq!(limits.fuel, 123);
		assert_eq!(limits.call_timeout, std::time::Duration::from_secs(7));
	}
}
