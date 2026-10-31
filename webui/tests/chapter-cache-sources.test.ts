import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

test('system settings put the cover cache above the chapter page cache', () => {
	const source = readFileSync(new URL('../src/pages/SettingsPage.tsx', import.meta.url), 'utf8');
	const cover = source.indexOf('<CoverCacheSection />');
	const chapter = source.indexOf('<ChapterCacheSection />');
	assert.ok(cover > 0, 'SettingsPage renders the cover cache section');
	assert.ok(chapter > 0, 'SettingsPage renders the chapter page cache section');
	assert.ok(cover < chapter, 'the cover cache is listed above the chapter page cache');
});

test('chapter cache source detail starts collapsed and expands to the per-source rows', async (context) => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	context.after(() => vite.close());
	const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
	await i18n.changeLanguage('en');
	const { ChapterCacheSources } = await vite.ssrLoadModule('/src/pages/settings/ChapterCacheSources.tsx');
	const breakdown = { pageBytes: 0, originalBytes: 0, blobBytes: 0, opdsBytes: 0 };
	const sources = [
		{ sourceId: 1, sourceName: 'Alpha', usedBytes: 2 * 1024 ** 2, chapterCount: 18, breakdown },
		{ sourceId: 2, sourceName: 'Beta', usedBytes: 1024, chapterCount: 4, breakdown }
	];
	const markup = renderToStaticMarkup(
		createElement(ChapterCacheSources, { sources, clearing: false, onClear: () => {} })
	);
	assert.match(markup, /^<details class="settings-cache__sources"><summary>By source \(2\)<\/summary>/);
	assert.doesNotMatch(markup, /<details[^>]*\bopen\b/);
	assert.match(markup, /<span class="settings-row__label">Alpha<\/span>/);
	assert.match(markup, /2 MiB used across 18 chapters/);
	assert.match(markup, /<span class="settings-row__label">Beta<\/span>/);
	assert.match(markup, /1 KiB used across 4 chapters/);
	assert.match(markup, />Clear source cache</);
	const empty = renderToStaticMarkup(
		createElement(ChapterCacheSources, { sources: [], clearing: false, onClear: () => {} })
	);
	assert.equal(empty, '');
});
