import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { Page } from '../src/api/types.ts';
import { DEFAULT_TYPOGRAPHY } from '../src/pages/reader/typography.ts';

function page(content: Page['content']): Page {
	return { content, thumbnail: null, hasDescription: false, description: null, index: 0 };
}

async function renderSettings(currentPage: Page | null): Promise<string> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		const previous = i18n.language;
		await i18n.changeLanguage('en');
		try {
			const { ReaderSettings } = await vite.ssrLoadModule('/src/pages/reader/ReaderSettings.tsx');
			return renderToStaticMarkup(
				createElement(ReaderSettings, {
					mode: 'paged',
					dir: 'ltr',
					onDirChange: () => {},
					currentPage,
					typography: DEFAULT_TYPOGRAPHY,
					onTypographyChange: () => {}
				})
			);
		} finally {
			await i18n.changeLanguage(previous);
		}
	} finally {
		await vite.close();
	}
}

test('image pages keep direction settings but do not offer typography controls', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /class="reader__settings card"/);
	assert.match(markup, /Reading direction/);
	assert.doesNotMatch(markup, /reader__typography/);
	assert.doesNotMatch(markup, /aria-label="Font"/);
	assert.doesNotMatch(markup, /type="range"/);
});

test('text pages keep typography controls', async () => {
	const markup = await renderSettings(page({ Text: 'A text chapter' }));
	assert.match(markup, /reader__typography/);
	assert.match(markup, /aria-label="Font"/);
	assert.match(markup, /Font size/);
	assert.match(markup, /Line height/);
});

test('description pages keep typography controls', async () => {
	const markup = await renderSettings({ ...page({ Url: ['https://example.invalid/page.png', null] }), hasDescription: true, description: 'Page description' });
	assert.match(markup, /reader__typography/);
	assert.match(markup, /aria-label="Font"/);
});
