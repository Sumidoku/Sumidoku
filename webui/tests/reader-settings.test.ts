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

async function renderSettings(currentPage: Page | null, mode: 'paged' | 'continuous' = 'paged'): Promise<string> {
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
					mode,
					dir: 'ltr',
					onDirChange: () => {},
					layout: 'single',
					onLayoutChange: () => {},
					tapZones: 'lr',
					onTapZonesChange: () => {},
					bg: 'dark',
					onBgChange: () => {},
					scale: 'screen',
					onScaleChange: () => {},
					split: false,
					onSplitChange: () => {},
					stretch: false,
					onStretchChange: () => {},
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

test('tap-zone and background controls render in paged mode', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /aria-label="Tap zones"/);
	assert.match(markup, />Kindle</);
	assert.match(markup, />Off</);
	assert.match(markup, /Background/);
	assert.match(markup, />Black</);
	assert.match(markup, />Gray</);
	assert.match(markup, />White</);
	assert.match(markup, />Follow system</);
});

test('continuous mode marks direction, layout and tap zones as paged-only', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }), 'continuous');
	assert.match(markup, /Reading direction \(paged mode only\)/);
	assert.match(markup, /Page layout \(paged mode only\)/);
	assert.match(markup, /Tap zones \(paged mode only\)/);
	// The background applies to every mode, so its title stays unmarked
	assert.doesNotMatch(markup, /Background \(paged mode only\)/);
});

test('page-scale select and stretch toggle render in paged mode', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /aria-label="Page scale"/);
	assert.match(markup, />Fit width \(shrink only\)</);
	assert.match(markup, />Original size</);
	assert.match(markup, /<input type="checkbox"/);
	assert.match(markup, /Stretch small pages/);
});

test('continuous mode marks page scale and stretch as paged-only', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }), 'continuous');
	assert.match(markup, /Page scale \(paged mode only\)/);
	assert.match(markup, /Stretch small pages \(paged mode only\)/);
});

test('the wide-page split toggle renders unmarked in paged single layout', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /Split wide pages/);
	assert.doesNotMatch(markup, /Split wide pages \(/);
});

test('continuous mode marks the split toggle as paged-only', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }), 'continuous');
	assert.match(markup, /Split wide pages \(paged mode only\)/);
});
