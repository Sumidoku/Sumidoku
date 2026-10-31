import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';

async function loadSpreads() {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		return await vite.ssrLoadModule('/src/pages/reader/spreads.ts');
	} finally {
		await vite.close();
	}
}

test('an image page without dimensions reserves a viewport-tall box', async () => {
	const { continuousPageClass } = await loadSpreads();
	assert.equal(continuousPageClass(true, null), 'reader__pg reader__pg--unsized');
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8');
	assert.match(css, /\.reader__pg--unsized,\n\.reader__pg--unsized \.reader__img--loading \{\n\tmin-height: 100dvh;/);
});

test('a measured page sizes itself, and text pages have their own box', async () => {
	const { continuousPageClass } = await loadSpreads();
	assert.equal(continuousPageClass(true, { width: 800, height: 1200 }), 'reader__pg');
	assert.equal(continuousPageClass(false, null), 'reader__pg');
});
