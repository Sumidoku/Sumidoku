//! send_partial_result 的端到端测试。
//! fixture 源在 tests/fixtures/partial-src/（no_std cdylib），重建方式：
//! `cd tests/fixtures/partial-src && cargo build --release --target wasm32-unknown-unknown`
//! 再把 target/wasm32-unknown-unknown/release/sumidoku_fixture_partial.wasm
//! 复制为 tests/fixtures/partial/main.wasm。
use aidoku_wasm::{
	DirectPolicy, HomePartial, NoopCookieSink, NoopDefaultsSink, SourceConfig, SourceHandle, StdoutLogger,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Semaphore, mpsc};

fn config() -> SourceConfig {
	SourceConfig {
		key: "en.partial".into(),
		wasm_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/partial/main.wasm"),
		limits: Default::default(),
		defaults: HashMap::new(),
		sink: Arc::new(NoopDefaultsSink),
		cookies: HashMap::new(),
		cookie_sink: Arc::new(NoopCookieSink),
		policy: Arc::new(DirectPolicy { user_agent: None }),
		logger: Arc::new(StdoutLogger),
	}
}

fn scroller_entries(component: &aidoku::HomeComponent) -> Vec<String> {
	let aidoku::HomeComponentValue::Scroller { entries, .. } = &component.value else {
		panic!("expected scroller component");
	};
	entries.iter().map(|l| l.title.clone()).collect()
}

#[tokio::test]
async fn home_partials_stream_in_order_before_final() {
	let (handle, features) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");
	assert!(features.home);

	let (tx, mut rx) = mpsc::unbounded_channel();
	let layout = handle.get_home_streaming(tx).await.expect("home layout");

	let mut events = Vec::new();
	while let Ok(event) = rx.try_recv() {
		events.push(event);
	}
	assert!(rx.recv().await.is_none(), "调用结束后 channel 应关闭");

	// fixture 序列：Layout 占位 → Component A → Component B → Component C（新增）
	assert_eq!(events.len(), 4, "partial 事件应按到达顺序全量送达");
	let HomePartial::Layout(placeholder) = &events[0] else {
		panic!("事件 0 应为 Layout 快照");
	};
	assert_eq!(placeholder.components.len(), 2);
	assert!(scroller_entries(&placeholder.components[0]).is_empty());

	for (event, index, title) in [(&events[1], 0, "Partial A"), (&events[2], 1, "Partial B"), (&events[3], 2, "Partial C")] {
		let HomePartial::Component { index: i, component } = event else {
			panic!("事件应为 Component");
		};
		assert_eq!(*i, index);
		assert_eq!(component.title.as_deref(), Some(title));
	}
	assert_eq!(scroller_entries(match &events[1] {
		HomePartial::Component { component, .. } => component,
		_ => unreachable!(),
	}), ["a-1"]);

	// 终态以导出返回为准，与 partial 不同：A 组件多一条 a-2
	assert_eq!(layout.components.len(), 3);
	assert_eq!(scroller_entries(&layout.components[0]), ["a-1", "a-2"]);
}

#[tokio::test]
async fn search_partials_are_ordered_manga_deltas() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");

	let (tx, mut rx) = mpsc::unbounded_channel();
	let result = handle
		.get_search_manga_list_streaming(None, 1, Vec::new(), tx)
		.await
		.expect("search result");

	let mut keys = Vec::new();
	while let Ok(manga) = rx.try_recv() {
		keys.push(manga.key);
	}
	assert_eq!(keys, ["partial-1", "partial-2", "partial-3"]);

	assert_eq!(result.entries.len(), 2);
	assert_eq!(result.entries[0].key, "final-1");
	assert!(result.has_next_page);
}

#[tokio::test]
async fn non_streaming_calls_discard_partials() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");

	// 无 sink 时 partial 按 ABI 丢弃，调用照常返回终态
	let layout = handle.get_home().await.expect("home layout");
	assert_eq!(layout.components.len(), 3);
	let result = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect("search result");
	assert_eq!(result.entries.len(), 2);
}
