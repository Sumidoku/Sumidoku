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

test('splitSides shows the leading half first in each direction', async () => {
	await withVite(async (vite) => {
		const { splitSides } = await vite.ssrLoadModule('/src/pages/reader/splitWide.ts');
		assert.deepEqual(splitSides('ltr'), ['left', 'right']);
		assert.deepEqual(splitSides('rtl'), ['right', 'left']);
	});
});

test('isSplittable only accepts strictly landscape dimensions', async () => {
	await withVite(async (vite) => {
		const { isSplittable } = await vite.ssrLoadModule('/src/pages/reader/splitWide.ts');
		assert.equal(isSplittable(null), false);
		assert.equal(isSplittable(undefined), false);
		assert.equal(isSplittable({ width: 1600, height: 800 }), true);
		assert.equal(isSplittable({ width: 800, height: 800 }), false);
		assert.equal(isSplittable({ width: 800, height: 1200 }), false);
	});
});

test('turnSplit walks halves of landscape pages and whole pages otherwise', async () => {
	await withVite(async (vite) => {
		const { turnSplit } = await vite.ssrLoadModule('/src/pages/reader/splitWide.ts');
		// page 1 is landscape, pages 0 and 2 are portrait
		const landscape = (i: number) => i === 1;
		// forward: into a landscape page -> first half, then second half, then next page
		assert.deepEqual(turnSplit({ page: 0, half: 0 }, 3, landscape, true), { page: 1, half: 0 });
		assert.deepEqual(turnSplit({ page: 1, half: 0 }, 3, landscape, true), { page: 1, half: 1 });
		assert.deepEqual(turnSplit({ page: 1, half: 1 }, 3, landscape, true), { page: 2, half: 0 });
		assert.deepEqual(turnSplit({ page: 2, half: 0 }, 3, landscape, true), null);
		// backward: leaving a landscape page backwards shows its second half first
		assert.deepEqual(turnSplit({ page: 2, half: 0 }, 3, landscape, false), { page: 1, half: 1 });
		assert.deepEqual(turnSplit({ page: 1, half: 1 }, 3, landscape, false), { page: 1, half: 0 });
		assert.deepEqual(turnSplit({ page: 1, half: 0 }, 3, landscape, false), { page: 0, half: 0 });
		assert.deepEqual(turnSplit({ page: 0, half: 0 }, 3, landscape, false), null);
	});
});

test('splitEndReached holds the read mark until the last half of a split final page', async () => {
	await withVite(async (vite) => {
		const { splitEndReached } = await vite.ssrLoadModule('/src/pages/reader/splitWide.ts');
		const landscape = (i: number) => i === 1;
		// final page is landscape: first half is not the end, second half is
		assert.equal(splitEndReached(1, 0, 2, landscape), false);
		assert.equal(splitEndReached(1, 1, 2, landscape), true);
		// final page is portrait: showing it at all is the end
		assert.equal(splitEndReached(0, 0, 1, () => false), true);
	});
});
