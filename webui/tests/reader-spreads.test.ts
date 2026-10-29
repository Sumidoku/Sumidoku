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

test('measured dimensions only fill gaps the server left', async () => {
	await withVite(async (vite) => {
		const { recordMeasured } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		const wide = { width: 2000, height: 1000 };
		// Server-known pages are never overridden by a measurement
		const withServer = recordMeasured({}, 0, wide, { width: 800, height: 1200 });
		assert.deepEqual(withServer, {});
		// Invalid measurements are rejected
		assert.deepEqual(recordMeasured({}, 0, { width: 0, height: 100 }, null), {});
		assert.deepEqual(recordMeasured({}, 0, { width: Number.NaN, height: 100 }, null), {});
		// A genuine gap is filled, and re-measuring the same value keeps the map identity
		const filled = recordMeasured({}, 1, wide, null);
		assert.deepEqual(filled, { 1: wide });
		assert.equal(recordMeasured(filled, 1, { ...wide }, null), filled);
	});
});

test('a measured landscape page re-pairs its spread', async () => {
	await withVite(async (vite) => {
		const { buildSpreads } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		// Three pages without known dimensions pair up as all-portrait…
		const unknown = () => false;
		assert.deepEqual(buildSpreads(3, unknown, 'doubleNoCover'), [[0, 1], [2, null]]);
		// …until a load measurement reveals page 1 as landscape, which then stands
		// alone and pushes page 0 onto a blank
		const measured = (i: number) => i === 1;
		assert.deepEqual(buildSpreads(3, measured, 'doubleNoCover'), [[0, null], [1], [2, null]]);
	});
});
