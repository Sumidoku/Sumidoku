//! Crash protection for deep recursion: recursion past the wasmi engine limit must trap
//! gracefully (not physically overflow the actor thread's stack and drag down the whole
//! process), and the instance must stay usable after the trap. The guest is self-recursive
//! WAT with a host call riding in every frame (the real shape of parser-style workloads)
use aidoku_wasm::{
	DirectPolicy, Error, Limits, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle,
	StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;

fn recursion_guest() -> PathBuf {
	let bytes = wat::parse_str(
		r#"(module
  (import "std" "current_date" (func $now (result f64)))
  (memory (export "memory") 1)
  ;; Valid ABI result (len=10 + cap=0 + postcard empty MangaPageResult [0,0]);
  ;; returned after the recursion, the host parses it as an empty list
  (data (i32.const 1024) "\0a\00\00\00\00\00\00\00\00\00")
  (func $rec (param i32) (result i32)
    (drop (call $now))
    (if (result i32) (i32.le_s (local.get 0) (i32.const 0))
      (then (i32.const 0))
      (else (i32.add (i32.const 1) (call $rec (i32.sub (local.get 0) (i32.const 1)))))))
  (func (export "start"))
  (func (export "get_manga_list") (param i32 i32) (result i32)
    (drop (call $rec (local.get 1)))
    (i32.const 1024)))"#,
	)
	.expect("valid wat");
	let dir = std::env::temp_dir().join(format!("aidoku-wasm-rec-limit-{}", std::process::id()));
	std::fs::create_dir_all(&dir).expect("create fixture dir");
	let path = dir.join("main.wasm");
	std::fs::write(&path, bytes).expect("write fixture wasm");
	path
}

async fn spawn() -> SourceHandle {
	let config = SourceConfig {
		key: "en.reclimit".to_string(),
		wasm_path: recursion_guest(),
		limits: Limits::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
	};
	let (handle, features) = SourceHandle::spawn(config, Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	assert!(features.manga_list);
	handle
}

fn listing() -> aidoku::Listing {
	aidoku::Listing {
		id: "hot".into(),
		name: "Hot".into(),
		kind: aidoku::ListingKind::List,
	}
}

#[tokio::test]
async fn deep_recursion_traps_gracefully_and_instance_survives() {
	let handle = spawn().await;
	// wasmi's default recursion limit is 1000: 2000 frames must trap instead of physically
	// overflowing the actor stack (#213: the source-zh.godamanga thread's stack overflow
	// aborted the whole process)
	let err = handle
		.get_manga_list(listing(), 2000)
		.await
		.expect_err("over-limit recursion must trap");
	match err {
		Error::Trap(msg) => assert_eq!(msg, "call stack exhausted"),
		other => panic!("expected stack-exhaustion trap, got {other}"),
	}
	// The instance is not dead after the trap: shallow calls work as usual
	handle
		.get_manga_list(listing(), 10)
		.await
		.expect("reused after trap");
}

#[tokio::test]
async fn deep_recursion_within_limit_completes() {
	let handle = spawn().await;
	// Depth within the limit must complete on the enlarged actor stack (the 2 MiB default stack used to physically overflow)
	handle
		.get_manga_list(listing(), 900)
		.await
		.expect("900-deep recursion completes");
}
