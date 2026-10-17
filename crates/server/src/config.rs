use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Config {
	pub addr: String,
	pub database_url: String,
	pub sources_dir: PathBuf,
	pub call_concurrency: usize,
	pub log: String,
	pub covers_dir: PathBuf,
	pub cover_cache_max: usize,
	pub webui_dir: PathBuf,
	pub user_agent: String,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			addr: "0.0.0.0:3000".into(),
			database_url: "sqlite://data/suzuri.db?mode=rwc".into(),
			sources_dir: PathBuf::from("./sources"),
			call_concurrency: 8,
			log: "info".into(),
			covers_dir: PathBuf::from("./data/covers"),
			cover_cache_max: 512,
			webui_dir: PathBuf::from("./webui/build"),
			user_agent: DEFAULT_USER_AGENT.into(),
		}
	}
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
	addr: Option<String>,
	database_url: Option<String>,
	sources_dir: Option<PathBuf>,
	call_concurrency: Option<usize>,
	log: Option<String>,
	covers_dir: Option<PathBuf>,
	cover_cache_max: Option<usize>,
	webui_dir: Option<PathBuf>,
	user_agent: Option<String>,
}

impl Config {
	/// 默认值 ← ./config.toml（可缺省）← 环境变量（SUZURI_ 前缀优先，DATABASE_URL 兜底）
	pub fn load() -> Self {
		let mut config = Self::default();
		if let Ok(raw) = std::fs::read_to_string("./config.toml") {
			match toml::from_str::<ConfigFile>(&raw) {
				Ok(file) => {
					if let Some(v) = file.addr {
						config.addr = v;
					}
					if let Some(v) = file.database_url {
						config.database_url = v;
					}
					if let Some(v) = file.sources_dir {
						config.sources_dir = v;
					}
					if let Some(v) = file.call_concurrency {
						config.call_concurrency = v;
					}
					if let Some(v) = file.log {
						config.log = v;
					}
					if let Some(v) = file.covers_dir {
						config.covers_dir = v;
					}
					if let Some(v) = file.cover_cache_max {
						config.cover_cache_max = v;
					}
					if let Some(v) = file.webui_dir {
						config.webui_dir = v;
					}
					if let Some(v) = file.user_agent {
						config.user_agent = v;
					}
				}
				Err(e) => eprintln!("invalid ./config.toml, using defaults: {e}"),
			}
		}
		if let Ok(v) = std::env::var("SUZURI_ADDR") {
			config.addr = v;
		}
		if let Ok(v) = std::env::var("DATABASE_URL") {
			config.database_url = v;
		}
		if let Ok(v) = std::env::var("SUZURI_DATABASE_URL") {
			config.database_url = v;
		}
		if let Ok(v) = std::env::var("SUZURI_SOURCES_DIR") {
			config.sources_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUZURI_CALL_CONCURRENCY") {
			if let Ok(n) = v.parse() {
				config.call_concurrency = n;
			}
		}
		if let Ok(v) = std::env::var("SUZURI_LOG") {
			config.log = v;
		}
		if let Ok(v) = std::env::var("SUZURI_COVERS_DIR") {
			config.covers_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUZURI_COVER_CACHE_MAX") {
			if let Ok(n) = v.parse() {
				config.cover_cache_max = n;
			}
		}
		if let Ok(v) = std::env::var("SUZURI_WEBUI_DIR") {
			config.webui_dir = PathBuf::from(v);
		}
		if let Ok(v) = std::env::var("SUZURI_USER_AGENT") {
			config.user_agent = v;
		}
		config
	}
}
