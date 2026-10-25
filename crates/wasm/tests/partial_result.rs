//! End-to-end tests for send_partial_result.
//! The fixture source lives in tests/fixtures/partial-src/ (no_std cdylib); rebuild with:
//! `cd tests/fixtures/partial-src && cargo build --release --target wasm32-unknown-unknown`
//! then copy target/wasm32-unknown-unknown/release/sumidoku_fixture_partial.wasm
//! over tests/fixtures/partial/main.wasm.
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
		accept_language: aidoku_wasm::DEFAULT_ACCEPT_LANGUAGE.to_string(),
		browser_trawl: None,
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

	// Fixture sequence: Layout placeholder → Component A → Component B → Component C (new)
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

	// The final state follows the export's return and differs from the partials: component A has one more entry, a-2
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
async fn manga_update_partials_are_full_snapshots_before_final() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");

	let (tx, mut rx) = mpsc::unbounded_channel();
	let input = aidoku::Manga {
		key: "manga-1".into(),
		..Default::default()
	};
	let updated = handle
		.get_manga_update_streaming(input, true, true, tx)
		.await
		.expect("manga update");

	let mut events = Vec::new();
	while let Ok(manga) = rx.try_recv() {
		events.push(manga);
	}
	assert!(rx.recv().await.is_none(), "调用结束后 channel 应关闭");

	// Fixture sequence: details snapshot (no chapters yet) → details+two-chapters snapshot; the input key is preserved as-is
	assert_eq!(events.len(), 2, "partial 事件应按到达顺序全量送达");
	assert_eq!(events[0].key, "manga-1");
	assert_eq!(events[0].title, "Partial Title");
	assert!(events[0].chapters.is_none(), "详情快照尚未带章节");
	let chapters = events[1].chapters.as_ref().expect("第二章快照");
	assert_eq!(chapters.len(), 2);
	assert_eq!(chapters[0].key, "c1");

	// The final state follows the export's return and differs from the partials: one more chapter, c3
	assert_eq!(updated.key, "manga-1");
	assert_eq!(updated.title, "Partial Title");
	let chapters = updated.chapters.as_ref().expect("终态章节");
	assert_eq!(chapters.len(), 3);
	assert_eq!(chapters[2].key, "c3");
}

#[tokio::test]
async fn manga_update_partials_respect_requested_parts() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");

	// Details only: just one partial, the details snapshot
	let (tx, mut rx) = mpsc::unbounded_channel();
	let input = aidoku::Manga {
		key: "manga-1".into(),
		..Default::default()
	};
	let updated = handle
		.get_manga_update_streaming(input, true, false, tx)
		.await
		.expect("details update");
	let mut events = Vec::new();
	while let Ok(manga) = rx.try_recv() {
		events.push(manga);
	}
	assert_eq!(events.len(), 1);
	assert!(events[0].chapters.is_none());
	assert!(updated.chapters.is_none());

	// Chapters only: just one partial, the chapters snapshot
	let (tx, mut rx) = mpsc::unbounded_channel();
	let input = aidoku::Manga {
		key: "manga-1".into(),
		..Default::default()
	};
	let updated = handle
		.get_manga_update_streaming(input, false, true, tx)
		.await
		.expect("chapters update");
	let mut events = Vec::new();
	while let Ok(manga) = rx.try_recv() {
		events.push(manga);
	}
	assert_eq!(events.len(), 1);
	assert_eq!(events[0].chapters.as_ref().map(|c| c.len()), Some(2));
	assert_eq!(updated.chapters.as_ref().map(|c| c.len()), Some(3));
}

#[tokio::test]
async fn non_streaming_calls_discard_partials() {
	let (handle, _) = SourceHandle::spawn(config(), Arc::new(Semaphore::new(8)))
		.await
		.expect("spawn source");

	// With no sink, partials are dropped per the ABI, and the call still returns the final state
	let layout = handle.get_home().await.expect("home layout");
	assert_eq!(layout.components.len(), 3);
	let result = handle
		.get_search_manga_list(None, 1, Vec::new())
		.await
		.expect("search result");
	assert_eq!(result.entries.len(), 2);
}
