import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

test('import modal titles evaluate translation calls', () => {
	const source = readFileSync(new URL('../src/pages/SettingsPage.tsx', import.meta.url), 'utf8');
	const quotedTitles = source.match(/title="\{t\([^"]*\}"/g) ?? [];
	assert.deepEqual(quotedTitles, []);
});

test('skipped items start collapsed with their summary and explanation visible on demand', async (context) => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	context.after(() => vite.close());
	const { SkippedItems } = await vite.ssrLoadModule('/src/pages/settings/SkippedItems.tsx');
	const markup = renderToStaticMarkup(
		createElement(SkippedItems, {
			label: 'Skipped items (2)',
			description: 'Matching source is not installed.',
			items: ['first', 'second']
		})
	);
	assert.equal(
		markup,
		'<details class="settings-backup__skipped"><summary>Skipped items (2)</summary>' +
			'<p class="settings-modal-text">Matching source is not installed.</p>' +
			'<ul class="settings-backup__warnings"><li>first</li><li>second</li></ul></details>'
	);
});
