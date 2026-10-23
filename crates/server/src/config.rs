use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct NetConfig {
	pub direct_domains: Vec<String>,
	pub relay_domains: Vec<String>,
	pub trawl_proxy: Option<String>,
	pub trawl_ca: Option<PathBuf>,
	pub proxy: Option<String>,
	pub allow_direct_fallback: bool,
	/// Adds the browser header suite (BROWSER_HEADERS) to direct requests: sec-fetch-* navigation semantics
	/// are mildly inaccurate for API/XHR, so disabling buys semantic honesty; passive bot checks verify the suite's presence, so disabling makes 403 more likely
	pub browser_headers: bool,
	/// trawl session API address (http://…); when unset, webview browser functions are explicitly unsupported
	pub browser_trawl: Option<String>,
}

impl Default for NetConfig {
	fn default() -> Self {
		Self {
			direct_domains: Vec::new(),
			relay_domains: Vec::new(),
			trawl_proxy: None,
			trawl_ca: None,
			proxy: None,
			allow_direct_fallback: false,
			browser_headers: true,
			browser_trawl: None,
		}
	}
}

#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
	/// Built-in admin's login username; None/blank = "admin"
	pub username: Option<String>,
	/// Site-wide password (shared by web UI login and /api/v1); None = auth disabled
	pub password: Option<String>,
	/// Long-lived API keys, used directly as Bearer tokens; plaintext in config, never stored in the DB
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
	/// env override (for headless) of the download.move_path setting; Some overrides the setting value
	pub download_move_path: Option<String>,
	pub update_interval_hours: u64,
	/// Scheduled refresh interval (hours) for source repo indexes; 0 = off, manual refresh only
	pub repo_refresh_interval_hours: u64,
	pub auto_download_new_chapters: bool,
	pub auto_download_limit: usize,
	/// Idle-eviction threshold (minutes) for source instances; <= 0 disables the sweeper
	pub source_idle_evict_minutes: i64,
	/// Process RSS threshold (MiB): above it, source instances are evicted idlest-first; 0 = monitoring off
	pub rss_threshold_mb: u64,
	pub wasm_memory_max_mb: u64,
	pub wasm_fuel: u64,
	pub wasm_call_timeout_secs: u64,
	/// Response body cap (MiB) for source endpoints: shared by the wasm net host and server-side fetches (covers/pages); 0 = unlimited
	pub net_body_max_mb: u64,
	/// Response body cap (MiB) for whole-chapter zip archives: a chapter packs N pages into one body, so it needs several times the per-image cap
	pub net_zip_body_max_mb: u64,
	/// The following four are env overrides (for headless) of the backup.* settings; Some overrides the setting values
	pub backup_enabled: Option<bool>,
	pub backup_interval_hours: Option<u64>,
	pub backup_retention_count: Option<u32>,
	/// Backup destination, allowed outside config_dir
	pub backup_dir: Option<PathBuf>,
	/// Local manga roots (scan targets of the built-in local source); None = [<config_dir>/local]
	pub local_dirs: Option<Vec<PathBuf>>,
	/// Name marking a oneshots directory for the local source; None/blank = the rule is off
	pub local_oneshots_dir: Option<String>,
	/// legacy: compatibility alias for auth.password, effective only when auth.password is absent
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
			download_move_path: None,
			update_interval_hours: 0,
			repo_refresh_interval_hours: 1,
			auto_download_new_chapters: false,
			auto_download_limit: 5,
			source_idle_evict_minutes: 30,
			rss_threshold_mb: 1024,
			wasm_memory_max_mb: 128,
			wasm_fuel: 10_000_000_000,
			wasm_call_timeout_secs: 60,
			net_body_max_mb: 32,
			net_zip_body_max_mb: 128,
			backup_enabled: None,
			backup_interval_hours: None,
			backup_retention_count: None,
			backup_dir: None,
			local_dirs: None,
			local_oneshots_dir: None,
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
	browser_headers: Option<bool>,
	browser_trawl: Option<String>,
}

impl Config {
	/// db, covers and sources all derive from config_dir, the single data-root knob
	fn derive_paths(&mut self) {
		self.database_url = format!("sqlite://{}/sumidoku.db?mode=rwc", self.config_dir.display());
		self.covers_dir = self.config_dir.join("covers");
	}

	/// Built-in admin's login username: the configured value (blank counts as unset), otherwise "admin"
	pub fn effective_username(&self) -> &str {
		self.auth
			.username
			.as_deref()
			.map(str::trim)
			.filter(|s| !s.is_empty())
			.unwrap_or("admin")
	}

	/// auth.password wins; when absent, legacy basic_password fills in so old configs keep working
	pub fn effective_password(&self) -> Option<&str> {
		self.auth.password.as_deref().or(self.basic_password.as_deref())
	}

	/// The source package directory is pinned under config_dir, not configurable
	pub fn sources_dir(&self) -> PathBuf {
		self.config_dir.join("sources")
	}

	/// Blob directory for page images materialized from PageContent::Image (<chapter_id>/<idx>.png), likewise pinned
	pub fn page_blobs_dir(&self) -> PathBuf {
		self.config_dir.join("page-blobs")
	}

	/// On-disk cache of OPDS source acquisition archives (named <chapter_id>); invalidated on page-row rewrite, self-healing after cache loss
	pub fn opds_cbz_dir(&self) -> PathBuf {
		self.config_dir.join("opds-cbz")
	}

	/// Default destination for scheduled backups; backup_dir (env) or the backup.dir setting can change it
	pub fn default_backups_dir(&self) -> PathBuf {
		self.config_dir.join("backups")
	}

	/// Local manga roots: explicit config wins (may live outside config_dir), otherwise pinned under config_dir
	pub fn local_dirs(&self) -> Vec<PathBuf> {
		self.local_dirs
			.clone()
			.unwrap_or_else(|| vec![self.config_dir.join("local")])
	}

	/// The oneshots-directory marker for the local source (a directory whose relative path contains it is
	/// never a manga; its direct archives are); a blank value counts as unset, so the feature stays off
	pub fn local_oneshots_dir(&self) -> Option<&str> {
		self.local_oneshots_dir
			.as_deref()
			.map(str::trim)
			.filter(|s| !s.is_empty())
	}

	/// Assembles the instance limits passed to the wasm runtime; the sleep/net/js/browser tiers are not configurable
	pub fn wasm_limits(&self) -> aidoku_wasm::Limits {
		aidoku_wasm::Limits {
			memory_max: self.wasm_memory_max_mb * 1024 * 1024,
			fuel: self.wasm_fuel,
			call_timeout: std::time::Duration::from_secs(self.wasm_call_timeout_secs),
			net_body_max: self.net_body_max_mb * 1024 * 1024,
			..aidoku_wasm::Limits::default()
		}
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AuthConfigFile {
	username: Option<String>,
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
	download_move_path: Option<String>,
	update_interval_hours: Option<u64>,
	repo_refresh_interval_hours: Option<u64>,
	auto_download_new_chapters: Option<bool>,
	auto_download_limit: Option<usize>,
	source_idle_evict_minutes: Option<i64>,
	rss_threshold_mb: Option<u64>,
	wasm_memory_max_mb: Option<u64>,
	wasm_fuel: Option<u64>,
	wasm_call_timeout_secs: Option<u64>,
	net_body_max_mb: Option<u64>,
	net_zip_body_max_mb: Option<u64>,
	backup_enabled: Option<bool>,
	backup_interval_hours: Option<u64>,
	backup_retention_count: Option<u32>,
	backup_dir: Option<PathBuf>,
	local_dirs: Option<Vec<PathBuf>>,
	/// legacy: one-item spelling of local_dirs, effective only when local_dirs is absent
	local_dir: Option<PathBuf>,
	local_oneshots_dir: Option<String>,
	basic_password: Option<String>,
	auth: Option<AuthConfigFile>,
	net: Option<NetConfigFile>,
}

impl Config {
	/// Defaults ← ./config.toml (optional) ← environment variables (SUMIDOKU_ prefix)
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
		if file.download_move_path.is_some() {
			self.download_move_path = file.download_move_path;
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
		if let Some(v) = file.source_idle_evict_minutes {
			self.source_idle_evict_minutes = v;
		}
		if let Some(v) = file.rss_threshold_mb {
			self.rss_threshold_mb = v;
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
		if let Some(v) = file.net_body_max_mb {
			self.net_body_max_mb = v;
		}
		if let Some(v) = file.net_zip_body_max_mb {
			self.net_zip_body_max_mb = v;
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
		if file.local_dirs.is_some() {
			self.local_dirs = file.local_dirs;
		} else if let Some(v) = file.local_dir {
			self.local_dirs = Some(vec![v]);
		}
		if let Some(v) = file.local_oneshots_dir {
			self.local_oneshots_dir = Some(v);
		}
		if let Some(v) = file.basic_password {
			self.basic_password = Some(v);
		}
		if let Some(auth) = file.auth {
			if auth.username.is_some() {
				self.auth.username = auth.username;
			}
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
			if net.browser_trawl.is_some() {
				self.net.browser_trawl = net.browser_trawl;
			}
			if let Some(v) = net.allow_direct_fallback {
				self.net.allow_direct_fallback = v;
			}
			if let Some(v) = net.browser_headers {
				self.net.browser_headers = v;
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
		if let Ok(v) = std::env::var("SUMIDOKU_DOWNLOAD_MOVE_PATH") {
			self.download_move_path = Some(v);
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
		if let Ok(v) = std::env::var("SUMIDOKU_SOURCE_IDLE_EVICT_MINUTES")
			&& let Ok(n) = v.parse()
		{
			self.source_idle_evict_minutes = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_RSS_THRESHOLD_MB")
			&& let Ok(n) = v.parse()
		{
			self.rss_threshold_mb = n;
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
		if let Ok(v) = std::env::var("SUMIDOKU_NET_BODY_MAX_MB")
			&& let Ok(n) = v.parse()
		{
			self.net_body_max_mb = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_ZIP_BODY_MAX_MB")
			&& let Ok(n) = v.parse()
		{
			self.net_zip_body_max_mb = n;
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
		// ';' is the list separator (paths may contain ','); empty segments are ignored, and a value
		// parsing to zero paths counts as unset so the single-item fallback still applies
		let local_dirs = std::env::var("SUMIDOKU_LOCAL_DIRS")
			.ok()
			.map(|v| {
				v.split(';')
					.map(|s| s.trim())
					.filter(|s| !s.is_empty())
					.map(PathBuf::from)
					.collect::<Vec<_>>()
			})
			.filter(|dirs| !dirs.is_empty());
		if let Some(dirs) = local_dirs {
			self.local_dirs = Some(dirs);
		} else if let Ok(v) = std::env::var("SUMIDOKU_LOCAL_DIR") {
			self.local_dirs = Some(vec![PathBuf::from(v)]);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_LOCAL_ONESHOTS_DIR") {
			self.local_oneshots_dir = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_BASIC_PASSWORD") {
			self.basic_password = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_AUTH_USERNAME") {
			self.auth.username = Some(v);
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
		if let Ok(v) = std::env::var("SUMIDOKU_NET_BROWSER_TRAWL") {
			self.net.browser_trawl = Some(v);
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_ALLOW_DIRECT_FALLBACK")
			&& let Ok(n) = v.parse()
		{
			self.net.allow_direct_fallback = n;
		}
		if let Ok(v) = std::env::var("SUMIDOKU_NET_BROWSER_HEADERS")
			&& let Ok(n) = v.parse()
		{
			self.net.browser_headers = n;
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
	fn browser_trawl_from_file() {
		let mut config = Config::default();
		assert_eq!(config.net.browser_trawl, None);
		let file: ConfigFile = toml::from_str("[net]\nbrowser_trawl = \"http://trawl:8191\"\n").expect("toml");
		config.apply_file(file);
		assert_eq!(config.net.browser_trawl.as_deref(), Some("http://trawl:8191"));
	}

	#[test]
	fn browser_trawl_from_env() {
		unsafe { std::env::set_var("SUMIDOKU_NET_BROWSER_TRAWL", "http://127.0.0.1:8191") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_NET_BROWSER_TRAWL") };
		assert_eq!(config.net.browser_trawl.as_deref(), Some("http://127.0.0.1:8191"));
	}

	#[test]
	fn auth_username_defaults_to_admin_and_blank_counts_as_unset() {
		assert_eq!(Config::default().effective_username(), "admin");
		let mut config = Config::default();
		config.auth.username = Some("  ".into());
		assert_eq!(config.effective_username(), "admin");
	}

	#[test]
	fn auth_username_from_file_and_env() {
		let file: ConfigFile = toml::from_str("[auth]\nusername = \"owner\"\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.effective_username(), "owner");

		unsafe { std::env::set_var("SUMIDOKU_AUTH_USERNAME", "env-owner") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_AUTH_USERNAME") };
		assert_eq!(config.effective_username(), "env-owner");
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
	fn rss_threshold_from_file_and_env() {
		assert_eq!(Config::default().rss_threshold_mb, 1024);
		let file: ConfigFile = toml::from_str("rss_threshold_mb = 256\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.rss_threshold_mb, 256);

		unsafe { std::env::set_var("SUMIDOKU_RSS_THRESHOLD_MB", "512") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_RSS_THRESHOLD_MB") };
		assert_eq!(config.rss_threshold_mb, 512);
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
		assert_eq!(limits.net_body_max, 32 * 1024 * 1024);
	}

	#[test]
	fn wasm_limits_mapping() {
		let config = Config {
			wasm_memory_max_mb: 64,
			wasm_fuel: 123,
			wasm_call_timeout_secs: 7,
			net_body_max_mb: 48,
			..Config::default()
		};
		let limits = config.wasm_limits();
		assert_eq!(limits.memory_max, 64 * 1024 * 1024);
		assert_eq!(limits.fuel, 123);
		assert_eq!(limits.call_timeout, std::time::Duration::from_secs(7));
		assert_eq!(limits.net_body_max, 48 * 1024 * 1024);
	}

	#[test]
	fn browser_headers_from_file_and_env() {
		assert!(Config::default().net.browser_headers, "默认开");
		let file: ConfigFile = toml::from_str("[net]\nbrowser_headers = false\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert!(!config.net.browser_headers);

		unsafe { std::env::set_var("SUMIDOKU_NET_BROWSER_HEADERS", "false") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_NET_BROWSER_HEADERS") };
		assert!(!config.net.browser_headers);
	}

	#[test]
	fn local_dirs_from_file() {
		assert_eq!(Config::default().local_dirs, None);
		let file: ConfigFile = toml::from_str("local_dirs = [\"/manga/a\", \"/manga/b\"]\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(
			config.local_dirs(),
			vec![PathBuf::from("/manga/a"), PathBuf::from("/manga/b")]
		);

		// the legacy single-dir key maps to a one-item list when local_dirs is absent
		let file: ConfigFile = toml::from_str("local_dir = \"/manga/local\"\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.local_dirs(), vec![PathBuf::from("/manga/local")]);

		// local_dirs wins when both spellings are present
		let file: ConfigFile =
			toml::from_str("local_dirs = [\"/manga/a\"]\nlocal_dir = \"/manga/local\"\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.local_dirs(), vec![PathBuf::from("/manga/a")]);
	}

	#[test]
	fn local_dirs_from_env() {
		// semicolon-separated, segments trimmed and empty ones ignored
		unsafe { std::env::set_var("SUMIDOKU_LOCAL_DIRS", "/manga/a; ;/manga/b;") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_LOCAL_DIRS") };
		assert_eq!(
			config.local_dirs(),
			vec![PathBuf::from("/manga/a"), PathBuf::from("/manga/b")]
		);

		// the single-dir variable is the one-item fallback
		unsafe { std::env::set_var("SUMIDOKU_LOCAL_DIR", "/manga/env-local") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_LOCAL_DIR") };
		assert_eq!(config.local_dirs(), vec![PathBuf::from("/manga/env-local")]);

		// DIRS wins over DIR, and a DIRS value with no real segment counts as unset
		unsafe {
			std::env::set_var("SUMIDOKU_LOCAL_DIRS", "/manga/a;/manga/b");
			std::env::set_var("SUMIDOKU_LOCAL_DIR", "/manga/env-local");
		}
		let mut config = Config::default();
		config.apply_env();
		assert_eq!(
			config.local_dirs(),
			vec![PathBuf::from("/manga/a"), PathBuf::from("/manga/b")]
		);
		unsafe { std::env::set_var("SUMIDOKU_LOCAL_DIRS", ";;") };
		let mut config = Config::default();
		config.apply_env();
		unsafe {
			std::env::remove_var("SUMIDOKU_LOCAL_DIRS");
			std::env::remove_var("SUMIDOKU_LOCAL_DIR");
		}
		assert_eq!(config.local_dirs(), vec![PathBuf::from("/manga/env-local")]);
	}

	#[test]
	fn local_dirs_default_under_config_dir() {
		let config = Config {
			config_dir: PathBuf::from("/data"),
			..Config::default()
		};
		assert_eq!(config.local_dirs(), vec![PathBuf::from("/data/local")]);
	}

	#[test]
	fn local_oneshots_dir_from_file_and_env() {
		assert_eq!(Config::default().local_oneshots_dir(), None);
		let file: ConfigFile = toml::from_str("local_oneshots_dir = \"oneshots\"\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.local_oneshots_dir(), Some("oneshots"));

		unsafe { std::env::set_var("SUMIDOKU_LOCAL_ONESHOTS_DIR", "OneShot") };
		let mut config = Config::default();
		config.apply_env();
		unsafe { std::env::remove_var("SUMIDOKU_LOCAL_ONESHOTS_DIR") };
		assert_eq!(config.local_oneshots_dir(), Some("OneShot"));

		// a blank value counts as unset: the feature is opt-in only
		let file: ConfigFile = toml::from_str("local_oneshots_dir = \"  \"\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.local_oneshots_dir(), None);
	}

	#[test]
	fn net_body_max_from_file_and_env() {
		let file: ConfigFile =
			toml::from_str("net_body_max_mb = 16\nnet_zip_body_max_mb = 96\n").expect("toml");
		let mut config = Config::default();
		config.apply_file(file);
		assert_eq!(config.net_body_max_mb, 16);
		assert_eq!(config.net_zip_body_max_mb, 96);

		unsafe {
			std::env::set_var("SUMIDOKU_NET_BODY_MAX_MB", "24");
			std::env::set_var("SUMIDOKU_NET_ZIP_BODY_MAX_MB", "160");
		}
		let mut config = Config::default();
		config.apply_env();
		unsafe {
			std::env::remove_var("SUMIDOKU_NET_BODY_MAX_MB");
			std::env::remove_var("SUMIDOKU_NET_ZIP_BODY_MAX_MB");
		}
		assert_eq!(config.net_body_max_mb, 24);
		assert_eq!(config.net_zip_body_max_mb, 160);
	}
}
