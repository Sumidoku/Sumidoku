import assert from 'node:assert/strict';
import test from 'node:test';
import { SseClient } from '../src/api/sse.ts';

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
			if (!handlers) {
				handlers = new Set();
				this.listeners.set(name, handlers);
			}
			handlers.add(handler);
		}

		close() {
			this.closed = true;
		}

		emit(name, data) {
			for (const handler of this.listeners.get(name) ?? []) handler({ data });
		}
	}

	globalThis.EventSource = MockEventSource;
	t.after(() => {
		if (original) Object.defineProperty(globalThis, 'EventSource', original);
		else delete globalThis.EventSource;
	});
	return instances;
}

test('shares one EventSource and dispatches only to the matching event handlers', (t) => {
	const instances = installMockEventSource(t);
	const client = new SseClient();
	const statuses = [];
	const downloads = [];
	const updates = [];
	const offStatus = client.onStatus((online) => statuses.push(online));
	assert.equal(instances.length, 0);

	const offDownload = client.on('download', (data) => downloads.push(data));
	const offSecondDownload = client.on('download', (data) => downloads.push(data));
	const offUpdate = client.on('update', (data) => updates.push(data));
	assert.equal(instances.length, 1);
	assert.equal(instances[0].url, '/api/v1/events');

	const download = { type: 'progress', chapterId: 7, downloaded: 2, total: 10 };
	const update = { type: 'job', status: 'running', mangaId: 3 };
	instances[0].emit('download', JSON.stringify(download));
	instances[0].emit('update', JSON.stringify(update));
	instances[0].emit('snapshot', JSON.stringify({ type: 'snapshot' }));
	assert.deepEqual(downloads, [download, download]);
	assert.deepEqual(updates, [update]);

	instances[0].onopen();
	instances[0].onerror();
	assert.deepEqual(statuses, [false, true, false]);
	offStatus();
	offDownload();
	offSecondDownload();
	offUpdate();
});

test('closes the EventSource when the last event handler is removed', (t) => {
	const instances = installMockEventSource(t);
	const client = new SseClient();
	let downloadCalls = 0;
	const offDownload = client.on('download', () => downloadCalls++);
	const offUpdate = client.on('update', () => {});
	const source = instances[0];

	offDownload();
	assert.equal(source.closed, false);
	source.emit('download', JSON.stringify({ type: 'progress' }));
	assert.equal(downloadCalls, 0);
	offUpdate();
	assert.equal(source.closed, true);
});

test('ignores malformed JSON and non-object messages', (t) => {
	const instances = installMockEventSource(t);
	const client = new SseClient();
	const received = [];
	client.on('download', (data) => received.push(data));
	const source = instances[0];

	for (const data of ['not-json', '', 'null', '[]', '1', '"text"', 'true', {}, undefined]) {
		source.emit('download', data);
	}
	assert.deepEqual(received, []);

	const valid = { type: 'finished', chapterId: 7 };
	source.emit('download', JSON.stringify(valid));
	assert.deepEqual(received, [valid]);
});
