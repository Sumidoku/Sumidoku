use aidoku_wasm::{SourceConfig, SourceHandle};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[tokio::test]
async fn std_env_aliases_are_registered() {
	let wat = r#"
	(module
	  (import "std" "abort" (func))
	  (import "std" "print" (func (param i32 i32)))
	  (import "std" "sleep" (func (param i32)))
	  (import "std" "send_partial_result" (func (param i32)))
	  (memory (export "memory") 1)
	  (func (export "start"))
	)"#;
	let dir = std::env::temp_dir().join(format!("aidoku-wasm-std-imports-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create fixture dir");
	let path = dir.join("main.wasm");
	std::fs::write(&path, wat::parse_str(wat).expect("valid wat")).expect("write fixture wasm");

	let config = SourceConfig::new("en.stdimports", path);
	let (_handle, _) = SourceHandle::spawn(config, Arc::new(Semaphore::new(1)))
		.await
		.expect("std aliases instantiate");
}
