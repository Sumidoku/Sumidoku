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

// Pages are all portrait unless listed here
const portrait = (landscape: number[] = []) => (i: number) => landscape.includes(i);

test('single layout keeps every page solo without blanks', async () => {
	await withVite(async (vite) => {
		const { buildSpreads } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		assert.deepEqual(buildSpreads(4, portrait(), 'single'), [[0], [1], [2], [3]]);
		assert.deepEqual(buildSpreads(0, portrait(), 'double'), []);
	});
});

test('double layout gives the cover and the closing page a blank mate', async () => {
	await withVite(async (vite) => {
		const { buildSpreads } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		// KMReader's convention: the cover pairs with a blank on its outer side and the
		// last page closes the book with its own blank, so 5 portrait pages make 4 spreads
		assert.deepEqual(buildSpreads(5, portrait(), 'double'), [[null, 0], [1, 2], [3, null], [4, null]]);
	});
});

test('double layout without a cover pairs pages straight through', async () => {
	await withVite(async (vite) => {
		const { buildSpreads } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		assert.deepEqual(buildSpreads(5, portrait(), 'doubleNoCover'), [[0, 1], [2, 3], [4, null]]);
		assert.deepEqual(buildSpreads(2, portrait(), 'doubleNoCover'), [[0, 1]]);
	});
});

test('landscape pages always stand alone and break a pair with a blank', async () => {
	await withVite(async (vite) => {
		const { buildSpreads } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		// Landscape cover and closing page stay solo instead of gaining a blank
		assert.deepEqual(buildSpreads(4, portrait([0, 3]), 'double'), [[0], [1, 2], [3]]);
		// A landscape second page pushes its would-be mate onto a blank
		assert.deepEqual(buildSpreads(3, portrait([1]), 'doubleNoCover'), [[0, null], [1], [2, null]]);
	});
});

test('spread lookup finds the containing spread and skips blank slots', async () => {
	await withVite(async (vite) => {
		const { buildSpreads, spreadIndexOf, spreadLeadPage } = await vite.ssrLoadModule(
			'/src/pages/reader/spreads.ts'
		);
		const spreads = buildSpreads(5, portrait(), 'double');
		assert.equal(spreadIndexOf(spreads, 0), 0);
		assert.equal(spreadIndexOf(spreads, 1), 1);
		assert.equal(spreadIndexOf(spreads, 2), 1);
		assert.equal(spreadIndexOf(spreads, 4), 3);
		assert.equal(spreadLeadPage(spreads[0]), 0);
		assert.equal(spreadLeadPage(spreads[3]), 4);
	});
});
