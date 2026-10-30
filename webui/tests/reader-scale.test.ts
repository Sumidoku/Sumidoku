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

test('stored page-scale values parse against a whitelist', async () => {
	await withVite(async (vite) => {
		const { parseScale } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		assert.equal(parseScale('width'), 'width');
		assert.equal(parseScale('widthShrink'), 'widthShrink');
		assert.equal(parseScale('original'), 'original');
		assert.equal(parseScale('WIDTH'), null);
		assert.equal(parseScale('stretch'), null);
		assert.equal(parseScale(null), null);
	});
});

test('nextScale cycles through every mode and wraps back to screen', async () => {
	await withVite(async (vite) => {
		const { SCALE_CYCLE, nextScale } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		let s = SCALE_CYCLE[0];
		assert.equal(s, 'screen');
		const seen = [s];
		for (let i = 0; i < SCALE_CYCLE.length - 1; i++) {
			s = nextScale(s);
			seen.push(s);
		}
		assert.deepEqual(seen, [...SCALE_CYCLE]);
		assert.equal(nextScale(s), 'screen');
	});
});
