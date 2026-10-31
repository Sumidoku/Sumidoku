import assert from 'node:assert/strict';
import test from 'node:test';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

function installMockEventSource(t) {
	const original = Object.getOwnPropertyDescriptor(globalThis, 'EventSource');
	const instances = [];
	class MockEventSource {
		constructor(url) {
			this.url = url;
			this.listeners = new Map();
			this.closed = false;
			this.onopen = null;
			this.onerror = null;
			instances.push(this);
		}
		addEventListener(name, handler) {
			let handlers = this.listeners.get(name);
			if (!handlers) this.listeners.set(name, (handlers = new Set()));
			handlers.add(handler);
		}
		close() { this.closed = true; }
		emit(name, data) {
			for (const handler of this.listeners.get(name) ?? []) handler({ data: JSON.stringify(data) });
		}
	}
	globalThis.EventSource = MockEventSource;
	t.after(() => {
		if (original) Object.defineProperty(globalThis, 'EventSource', original);
		else delete globalThis.EventSource;
	});
	return instances;
}

async function makeVite() {
	return createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
}

async function until(predicate) {
	for (let attempt = 0; attempt < 1000; attempt++) {
		if (predicate()) return;
		await new Promise((resolve) => setImmediate(resolve));
	}
	assert.fail('timed out waiting for query refetch');
}

test('authenticated stream leases share one connection and stop it after the last owner releases', async (t) => {
	const instances = installMockEventSource(t);
	const vite = await makeVite();
	try {
		const { acquireSseConnection } = await vite.ssrLoadModule('/src/api/sse.ts');
		const disabled = acquireSseConnection(false);
		assert.equal(instances.length, 0);
		const app = acquireSseConnection(true);
		const reader = acquireSseConnection(true);
		assert.equal(instances.length, 1);
		app();
		assert.equal(instances[0].closed, false);
		reader();
		assert.equal(instances[0].closed, true);
		disabled();
	} finally {
		await vite.close();
	}
});

test('finished downloads, snapshots and reconnects refetch active page/cache queries; progress does not', async (t) => {
	const instances = installMockEventSource(t);
	const vite = await makeVite();
	const qc = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 30_000 } } });
	const observers = [];
	let release: (() => void) | undefined;
	let unsubscribe: (() => void) | undefined;
	try {
		const { subscribeChapterCacheEvents, qk } = await vite.ssrLoadModule('/src/api/hooks.ts');
		const { acquireSseConnection } = await vite.ssrLoadModule('/src/api/sse.ts');
		const queries = new Map();
		for (const key of [['chapter', 7, 'pages'], ['chapter', 8, 'pages'], qk.chapterCache]) {
			let requests = 0;
			qc.setQueryData(key, { seeded: true });
			const observer = new QueryObserver(qc, {
				queryKey: key,
				queryFn: async () => ++requests,
				staleTime: 30_000
			});
			observers.push(observer.subscribe(() => {}));
			queries.set(JSON.stringify(key), () => requests);
		}
		release = acquireSseConnection(true);
		unsubscribe = subscribeChapterCacheEvents(qc);
		const source = instances[0];
		assert.ok(source);
		source.onopen(); // Initial connection is also a recovery boundary.
		await until(() => queries.get('["chapter",7,"pages"]')() === 1
			&& queries.get('["chapter",8,"pages"]')() === 1
			&& queries.get('["chapter-cache"]')() === 1);

		const callsBeforeProgress = [...queries.values()].map((read) => read());
		source.emit('download', { type: 'progress', chapterId: 7, downloaded: 1, total: 3 });
		await new Promise((resolve) => setImmediate(resolve));
		assert.deepEqual([...queries.values()].map((read) => read()), callsBeforeProgress);
		source.emit('download', { type: 'finished', chapterId: 7, downloaded: 3, total: 3 });
		await until(() => queries.get('["chapter",7,"pages"]')() === 2
			&& queries.get('["chapter-cache"]')() === 2);
		assert.equal(queries.get('["chapter",8,"pages"]')(), 1);

		source.onerror();
		source.onopen();
		await until(() => queries.get('["chapter",7,"pages"]')() === 3
			&& queries.get('["chapter",8,"pages"]')() === 2
			&& queries.get('["chapter-cache"]')() === 3);

		source.emit('snapshot', { type: 'snapshot', downloads: [] });
		await until(() => queries.get('["chapter",7,"pages"]')() === 4
			&& queries.get('["chapter",8,"pages"]')() === 3
			&& queries.get('["chapter-cache"]')() === 4);

		unsubscribe();
		const callsAfterCleanup = [...queries.values()].map((read) => read());
		source.emit('download', { type: 'finished', chapterId: 7, downloaded: 3, total: 3 });
		source.emit('snapshot', { type: 'snapshot', downloads: [] });
		source.onerror();
		source.onopen();
		await new Promise((resolve) => setImmediate(resolve));
		assert.deepEqual([...queries.values()].map((read) => read()), callsAfterCleanup);
		unsubscribe();
		unsubscribe = undefined;
		release();
		release = undefined;
		assert.equal(source.closed, true);
	} finally {
		unsubscribe?.();
		release?.();
		for (const unsubscribe of observers) unsubscribe();
		qc.clear();
		await vite.close();
	}
});
