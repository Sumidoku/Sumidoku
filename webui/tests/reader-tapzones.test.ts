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

test('the rl layout mirrors the page-turn sides in both directions', async () => {
	await withVite(async (vite) => {
		const { zoneAction } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		// lr: forward follows the reading direction (right under ltr, left under rtl)
		assert.equal(zoneAction('ltr', 'lr', 'right'), 'next');
		assert.equal(zoneAction('ltr', 'lr', 'left'), 'prev');
		assert.equal(zoneAction('rtl', 'lr', 'left'), 'next');
		assert.equal(zoneAction('rtl', 'lr', 'right'), 'prev');
		// rl: the turn sides swap, so rl+rtl moves forward on the right again
		assert.equal(zoneAction('ltr', 'rl', 'left'), 'next');
		assert.equal(zoneAction('ltr', 'rl', 'right'), 'prev');
		assert.equal(zoneAction('rtl', 'rl', 'right'), 'next');
		assert.equal(zoneAction('rtl', 'rl', 'left'), 'prev');
		// The mid zone always summons the toolbar, and off disables everything
		assert.equal(zoneAction('ltr', 'kindle', 'mid'), 'menu');
		assert.equal(zoneAction('ltr', 'off', 'left'), 'none');
		assert.equal(zoneAction('ltr', 'off', 'mid'), 'none');
	});
});

test('zoneAt hit-tests match the rendered zone geometry', async () => {
	await withVite(async (vite) => {
		const { zoneAt } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		// lr/rl: 28% turn zones on the sides
		assert.equal(zoneAt('lr', 0.1, 0.5), 'left');
		assert.equal(zoneAt('lr', 0.9, 0.5), 'right');
		assert.equal(zoneAt('lr', 0.5, 0.5), 'mid');
		assert.equal(zoneAt('rl', 0.1, 0.5), 'left');
		// edge: thin 15% turn zones
		assert.equal(zoneAt('edge', 0.1, 0.5), 'left');
		assert.equal(zoneAt('edge', 0.2, 0.5), 'mid');
		assert.equal(zoneAt('edge', 0.9, 0.5), 'right');
		// kindle: full-width top menu strip, then 75/25 prev/next
		assert.equal(zoneAt('kindle', 0.5, 0.1), 'mid');
		assert.equal(zoneAt('kindle', 0.1, 0.1), 'mid');
		assert.equal(zoneAt('kindle', 0.5, 0.5), 'left');
		assert.equal(zoneAt('kindle', 0.9, 0.5), 'right');
		// off: every tap falls back to the menu so the toolbar stays reachable
		assert.equal(zoneAt('off', 0.1, 0.5), 'mid');
		assert.equal(zoneAt('off', 0.9, 0.5), 'mid');
	});
});

test('stored tap-zone and background values parse against a whitelist', async () => {
	await withVite(async (vite) => {
		const { parseTapZones, parseBackground } = await vite.ssrLoadModule('/src/pages/reader/readerPrefs.ts');
		assert.equal(parseTapZones('kindle'), 'kindle');
		assert.equal(parseTapZones('off'), 'off');
		assert.equal(parseTapZones('LEFT_RIGHT'), null);
		assert.equal(parseTapZones(null), null);
		assert.equal(parseBackground('gray'), 'gray');
		// "auto" follows the app theme's resolved brightness
		assert.equal(parseBackground('auto'), 'auto');
		assert.equal(parseBackground(null), null);
	});
});
