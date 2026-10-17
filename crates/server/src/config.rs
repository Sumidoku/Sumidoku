use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
	pub addr: String,
	pub database_url: String,
	pub sources_dir: PathBuf,
	pub call_concurrency: usize,
	pub log: String,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			addr: "0.0.0.0:3000".into(),
			database_url: "sqlite://data/suzuri.db?mode=rwc".into(),
			sources_dir: PathBuf::from("./sources"),
			call_concurrency: 8,
			log: "info".into(),
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
		config
	}
}
