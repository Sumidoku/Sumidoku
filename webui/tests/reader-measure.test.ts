import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';

async function withVite<T>(run: (vite: ViteDevServer) => Promise<T>): Promise<T> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		return await run(vite);
	} finally {
		await vite.close();
	}
}

interface FakeImage {
	naturalWidth: number;
	naturalHeight: number;
	complete: boolean;
}

function fakeScheduler(): { queue: Array<() => void>; schedule: (cb: () => void) => void; pump: (n: number) => void } {
	const queue: Array<() => void> = [];
	return {
		queue,
		schedule: (cb) => queue.push(cb),
		pump: (n) => {
			for (let i = 0; i < n && queue.length > 0; i++) {
				const cb = queue.shift();
				cb?.();
			}
		}
	};
}

test('the poll reports dimensions as soon as the header parses', async () => {
	await withVite(async (vite) => {
		const { measureWhenKnown } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		const img: FakeImage = { naturalWidth: 0, naturalHeight: 0, complete: false };
		const { schedule, pump, queue } = fakeScheduler();
		const reports: Array<[number, number]> = [];
		measureWhenKnown(img, (w, h) => reports.push([w, h]), schedule);
		pump(3);
		assert.deepEqual(reports, []);
		// The header arrives: naturalWidth/naturalHeight settle before load (complete stays false)
		img.naturalWidth = 2000;
		img.naturalHeight = 1000;
		pump(1);
		assert.deepEqual(reports, [[2000, 1000]]);
		assert.equal(queue.length, 0, 'polling stops after the report');
	});
});

test('a stalled load cannot poll forever', async () => {
	await withVite(async (vite) => {
		const { measureWhenKnown } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		const img: FakeImage = { naturalWidth: 0, naturalHeight: 0, complete: false };
		const { schedule, pump, queue } = fakeScheduler();
		const reports: Array<[number, number]> = [];
		measureWhenKnown(img, (w, h) => reports.push([w, h]), schedule, 5);
		pump(10);
		assert.deepEqual(reports, []);
		assert.equal(queue.length, 0, 'the cap ends polling');
	});
});

test('a completed image without dimensions stops polling (load handler reports instead)', async () => {
	await withVite(async (vite) => {
		const { measureWhenKnown } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		const img: FakeImage = { naturalWidth: 0, naturalHeight: 0, complete: true };
		const { schedule, pump, queue } = fakeScheduler();
		measureWhenKnown(img, () => assert.fail('nothing to report'), schedule);
		pump(3);
		assert.equal(queue.length, 0, 'no further frames are scheduled');
	});
});
