import assert from 'node:assert/strict';
import test from 'node:test';
import { QueryClient } from '@tanstack/react-query';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';

async function makeVite() {
	return createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
}

function installMockEventSource(t) {
	const original = Object.getOwnPropertyDescriptor(globalThis, 'EventSource');
	const instances = [];

	class MockEventSource {
		constructor() {
			this.listeners = new Map();
			instances.push(this);
		}

		addEventListener(name, handler) {
			let handlers = this.listeners.get(name);
			if (!handlers) {
				handlers = new Set();
				this.listeners.set(name, handlers);
			}
			handlers.add(handler);
		}

		emit(name, data) {
			for (const handler of this.listeners.get(name) ?? []) handler({ data });
		}

		close() {}
	}

	globalThis.EventSource = MockEventSource;
	t.after(() => {
		if (original) Object.defineProperty(globalThis, 'EventSource', original);
		else delete globalThis.EventSource;
	});
	return instances;
}

function task(status: string) {
	return {
		id: 'local_scan:1',
		kind: 'local_scan',
		sourceId: 1,
		payload: {},
		priority: 4,
		status,
		createdAt: 1,
		startedAt: 1,
		finishedAt: status === 'done' ? 2 : null,
		error: null,
		result: null
	};
}

test('terminal local tasks refresh only their own local source views, never remote searches', async (t) => {
	const instances = installMockEventSource(t);
	const vite = await makeVite();
	const qc = new QueryClient();
	let unsubscribe: (() => void) | undefined;
	t.after(() => {
		unsubscribe?.();
		qc.clear();
	});
	try {
		const { qk, subscribeTaskEvents } = await vite.ssrLoadModule('/src/api/hooks.ts');
		unsubscribe = subscribeTaskEvents(qc);

		const keys = {
			tasks: qk.tasks,
			library: qk.library,
			manga: ['manga', 42],
			trash: qk.localTrash,
			installedSources: qk.sources,
			globalSearch: ['global-search', 'needle', [1, 2]],
			localSearch: ['sources', 1, 'search', { query: 'needle', filters: [] }],
			localListing: ['sources', 1, 'listing', 'latest'],
			localHome: ['sources', 1, 'home'],
			remoteSearch: ['sources', 2, 'search', { query: 'needle', filters: [] }],
			remoteListing: ['sources', 2, 'listing', 'latest'],
			remoteHome: ['sources', 2, 'home']
		} as const;
		for (const key of Object.values(keys)) qc.setQueryData(key, {});

		instances[0].emit('task', JSON.stringify(task('pending')));
		await Promise.resolve();
		assert.equal(qc.getQueryState(keys.tasks)?.isInvalidated, true);
		assert.equal(qc.getQueryState(keys.globalSearch)?.isInvalidated, false);

		instances[0].emit('task', JSON.stringify(task('done')));
		await Promise.resolve();
		for (const key of [keys.library, keys.manga, keys.trash, keys.localSearch, keys.localListing, keys.localHome]) {
			assert.equal(qc.getQueryState(key)?.isInvalidated, true, `${key.join('/')} should refresh`);
		}
		for (const key of [keys.installedSources, keys.globalSearch, keys.remoteSearch, keys.remoteListing, keys.remoteHome]) {
			assert.equal(qc.getQueryState(key)?.isInvalidated, false, `${key.join('/')} must not refetch`);
		}
	} finally {
		await vite.close();
	}
});
