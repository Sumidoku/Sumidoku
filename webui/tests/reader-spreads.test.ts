import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
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

test('spreadLastPage follows the last real page, so a paired final spread reaches the chapter end (#629)', async () => {
	await withVite(async (vite) => {
		const { buildSpreads, spreadLastPage, spreadLeadPage } = await vite.ssrLoadModule(
			'/src/pages/reader/spreads.ts'
		);
		// #629: doubleNoCover with an even page count pairs the final spread; its
		// lead page is the second-to-last page, so a landing-page progress report
		// never reaches the chapter end — the spread's last page is the last page
		const even = buildSpreads(4, portrait(), 'doubleNoCover');
		assert.deepEqual(even, [[0, 1], [2, 3]]);
		assert.equal(spreadLeadPage(even[1]), 2);
		assert.equal(spreadLastPage(even[1]), 3);
		// Blank mates (cover, closing page, broken pair) resolve to their only page
		const dbl = buildSpreads(5, portrait(), 'double');
		assert.equal(spreadLastPage(dbl[0]), 0);
		assert.equal(spreadLastPage(dbl[2]), 3);
		assert.equal(spreadLastPage(dbl[3]), 4);
		// A full pair resolves to its second page; a landscape solo to itself
		assert.equal(spreadLastPage([1, 2]), 2);
		assert.equal(spreadLastPage([7]), 7);
	});
});

test('a forward spread turn reports progress from the spread\'s last page, not the landing page (#629)', () => {
	// Source-reading wiring pin; CRLF checkouts normalise before matching
	const source = readFileSync(fileURLToPath(new URL('../src/pages/ReaderPage.tsx', import.meta.url)), 'utf8').replace(
		/\r\n/g,
		'\n'
	);
	// The turn still lands on the spread's lead page…
	assert.match(source, /const lead = spreadLeadPage\(target\);\n\t\t\t\t\tgotoPage\(lead\);/);
	// …but progress and the chapter-end mark follow the spread's last page, or a
	// paired final spread (doubleNoCover, even page count) never marks the chapter read
	assert.match(source, /const last = spreadLastPage\(target\);\n\t\t\t\t\tif \(last !== lead\) scheduleProgress\(last, last >= total - 1\);/);
});

test('initialEndProgress reports the end only when the opening unit already shows the last page (#629 heal)', async () => {
	await withVite(async (vite) => {
		const { buildSpreads, initialEndProgress, spreadIndexOf } = await vite.ssrLoadModule(
			'/src/pages/reader/spreads.ts'
		);
		// Resuming onto a paired final spread (doubleNoCover, even page count) sees
		// both pages, so the end is reported from the spread's last page
		const even = buildSpreads(4, portrait(), 'doubleNoCover'); // [[0,1],[2,3]]
		assert.equal(initialEndProgress(4, 2, even[spreadIndexOf(even, 2)], false), 3);
		assert.equal(initialEndProgress(4, 3, even[spreadIndexOf(even, 3)], false), 3);
		// A mid-chapter resume reports nothing
		assert.equal(initialEndProgress(4, 0, even[spreadIndexOf(even, 0)], false), null);
		// Single-page mode: only the last page itself qualifies, and a split wide
		// final page opening on its first half is not the end yet
		assert.equal(initialEndProgress(4, 3, null, false), 3);
		assert.equal(initialEndProgress(4, 2, null, false), null);
		assert.equal(initialEndProgress(4, 3, null, true), null);
		// A trailing blank mate does not hide the real last page
		assert.equal(initialEndProgress(5, 4, [4, null], false), 4);
		// A one-page chapter opens already complete; an empty chapter never does
		assert.equal(initialEndProgress(1, 0, [null, 0], false), 0);
		assert.equal(initialEndProgress(0, 0, null, false), null);
	});
});

test('opening the reader on the final visible unit reports the chapter end (resume heals #629)', () => {
	// Source-reading wiring pin; CRLF checkouts normalise before matching
	const source = readFileSync(fileURLToPath(new URL('../src/pages/ReaderPage.tsx', import.meta.url)), 'utf8').replace(
		/\r\n/g,
		'\n'
	);
	// Text chapters (column pagination from 0) are excluded at the call site…
	assert.match(source, /if \(mode === 'paged' && !textChapter\) \{/);
	// …the opening unit is the spread the initial page lands in (null in single-page mode)…
	assert.match(source, /const unit = spreads \? spreads\[spreadIndexOf\(spreads, initial\)\] : null;/);
	// …and a split wide final page's first half withholds the end mark
	assert.match(source, /const endPage = initialEndProgress\(pages\.length, initial, unit, splitOn && landscapeOf\(initial\)\);/);
	assert.match(source, /if \(endPage !== null\) scheduleProgress\(endPage, true\);/);
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

test('slotAspectRatio reserves a 2:3 box until the real dimensions arrive', async () => {
	await withVite(async (vite) => {
		const { slotAspectRatio } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		// Unknown dimensions fall back to the 2:3 box so an unloaded page stays in its half
		assert.equal(slotAspectRatio(null), '2 / 3');
		assert.equal(slotAspectRatio(undefined), '2 / 3');
		// Measured or server-reported dimensions take over the box
		assert.equal(slotAspectRatio({ width: 1400, height: 2100 }), '1400 / 2100');
	});
});

test('trackOffsetPercent slides one spread per index and flips the sign under rtl', async () => {
	await withVite(async (vite) => {
		const { trackOffsetPercent } = await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
		assert.equal(trackOffsetPercent('ltr', 0), 0);
		assert.equal(trackOffsetPercent('ltr', 3), -300);
		assert.equal(trackOffsetPercent('rtl', 0), 0);
		// Under rtl the track row is reversed, so a later spread slides in from the left
		assert.equal(trackOffsetPercent('rtl', 3), 300);
	});
});
