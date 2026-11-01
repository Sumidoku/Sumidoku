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

async function renderSettings(
	currentPage: Page | null,
	mode: 'paged' | 'continuous' = 'paged',
	layout: 'single' | 'double' = 'single'
): Promise<string> {
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
					layout,
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
					anim: true,
					anim: true,
					onAnimChange: () => {},
					pageTurn: 'auto',
					onPageTurnChange: () => {},
					continuousScale: 'width',
					onContinuousScaleChange: () => {},
					continuousPadding: 0,
					onContinuousPaddingChange: () => {},
					continuousGap: 0,
					onContinuousGapChange: () => {},
					transition: true,
					onTransitionChange: () => {},
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
	assert.match(markup, /reader__tz-preview/);
	assert.match(markup, /Page turn style/);
	assert.match(markup, />Fade</);
	assert.match(markup, /Background/);
	assert.match(markup, /aria-label="Black"/);
	assert.match(markup, /aria-label="Gray"/);
	assert.match(markup, /aria-label="White"/);
	assert.match(markup, /aria-label="Follow system"/);
});

test('continuous mode marks the page and turning groups as paged-only and locks them', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }), 'continuous');
	assert.equal(markup.match(/Paged mode only/g)?.length, 2);
	assert.equal(markup.match(/reader__settings-body--off" inert=""/g)?.length, 2);
	// The background applies to every mode, so its group stays unmarked and live
	const appearance = markup.slice(markup.indexOf('Continuous reading'));
	assert.doesNotMatch(appearance, /Paged mode only|inert/);
	assert.match(appearance, /aria-label="Background"/);
	assert.match(appearance, /aria-label="Page scale"/);
	assert.match(appearance, /Side padding/);
	assert.match(appearance, /Page gap/);
});

test('page-scale select and stretch toggle render in paged mode', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /aria-label="Page layout"/);
	assert.match(markup, /aria-label="Page scale"/);
	assert.match(markup, />Fit width \(shrink only\)</);
	assert.match(markup, />Original size</);
	assert.match(markup, /<input type="checkbox"/);
	assert.match(markup, /Stretch small pages/);
});

test('toggles use the shared switch', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.equal(markup.match(/type="checkbox"/g)?.length, 4);
	assert.equal(markup.match(/class="switch"/g)?.length, 4);
});

test('groups separate page, turning and appearance settings', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	const titles = [...markup.matchAll(/reader__settings-title">([^<]+)</g)].map((m) => m[1]);
	assert.deepEqual(titles, ['Page', 'Page turning', 'Appearance']);
	for (const label of ['Page layout', 'Reading direction', 'Background']) {
		assert.match(markup, new RegExp(`role="group" aria-label="${label}"`));
	}
});

test('paged mode leaves every group unmarked and live', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.doesNotMatch(markup, /Paged mode only|inert|\(single-page layout only\)/);
	assert.doesNotMatch(markup, /class="switch"[^>]*disabled/);
});

test('double layouts lock only the split toggle and say why', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }), 'paged', 'double');
	assert.match(markup, /reader__settings-row--off"><span>Split wide pages \(single-page layout only\)/);
	assert.equal(markup.match(/disabled=""/g)?.length, 1);
});
