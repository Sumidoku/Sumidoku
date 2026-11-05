import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { Page } from '../src/api/types.ts';
import { DEFAULT_FILTERS } from '../src/pages/reader/filters.ts';
import { DEFAULT_TYPOGRAPHY } from '../src/pages/reader/typography.ts';

function page(content: Page['content']): Page {
	return { content, thumbnail: null, hasDescription: false, description: null, index: 0 };
}

async function renderSettings(
	currentPage: Page | null,
	mode: 'paged' | 'continuous' = 'paged',
	layout: 'single' | 'double' = 'single',
	textOnly = false,
	textChapter = textOnly
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
					onModeChange: () => {},
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
					textOnly,
					textChapter,
					typography: DEFAULT_TYPOGRAPHY,
					onTypographyChange: () => {},
					filters: DEFAULT_FILTERS,
					onFiltersChange: () => {}
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

test('reading mode moves from the toolbar into the theme-aware settings panel', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.match(markup, /role="group" aria-label="Reading mode"/);
	assert.match(markup, />Paged</);
	assert.match(markup, />Scroll</);
	assert.match(markup, />Webtoon</);
	assert.doesNotMatch(markup, /data-theme=/);
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /onModeChange=\{setMode\}/);
	assert.doesNotMatch(source, /<SegmentedControl/);
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

test('tap previews use the page aspect ratio and HTML legends share the SVG colors', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	assert.equal(markup.match(/viewBox="0 0 20 28"/g)?.length, 5);
	assert.ok(Math.abs(39.1 / 27.642857 - 1.414) < 0.001);
	assert.match(markup, /<svg class="reader__tz-swatch"[^>]*><rect width="1" height="1" class="reader__tz-fill--prev"><\/rect><\/svg>Previous/);
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8');
	assert.match(css, /\.reader__tz-preview\s*\{[^}]*width: 27\.642857px;[^}]*height: 39\.1px;/s);
	assert.match(css, /\.reader__tz-swatch\s*\{[^}]*width: 8px;[^}]*height: 8px;/s);
	assert.match(css, /\.reader__tz-fill--prev\s*\{[^}]*--tz-color: #[0-9a-f]+;[^}]*fill: var\(--tz-color\)/s);
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
	// 4 page/turning toggles plus 8 filter switches (4 levels, sepia, grayscale, invert, tint)
	assert.equal(markup.match(/type="checkbox"/g)?.length, 12);
	assert.equal(markup.match(/class="switch"/g)?.length, 12);
});

test('groups separate page, turning and appearance settings', async () => {
	const markup = await renderSettings(page({ Url: ['https://example.invalid/page.png', null] }));
	const titles = [...markup.matchAll(/reader__settings-title">([^<]+)</g)].map((m) => m[1]);
	assert.deepEqual(titles, ['Reading mode', 'Page', 'Page turning', 'Filters', 'Appearance']);
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
	// Count disabled switches only: the filters reset button is also disabled while
	// every filter is off, and it is not a switch
	assert.equal(markup.match(/class="switch"[^>]*disabled/g)?.length, 1);
});

test('text-only chapters hide image-specific options', async () => {
	const markup = await renderSettings(page({ Text: 'A text chapter' }), 'paged', 'single', true);
	assert.doesNotMatch(markup, /aria-label="Page layout"/);
	assert.doesNotMatch(markup, /aria-label="Page scale"/);
	assert.doesNotMatch(markup, /Split wide pages/);
	assert.doesNotMatch(markup, /Stretch small pages/);
	assert.doesNotMatch(markup, /aria-label="Tap zones"/);
	assert.doesNotMatch(markup, /Page turn style/);
	assert.doesNotMatch(markup, /reader__tz-preview/);
	assert.doesNotMatch(markup, /reader__settings-title">Filters</);
});

test('text-only chapters keep direction, animation, transition, background and typography controls', async () => {
	const markup = await renderSettings(page({ Text: 'A text chapter' }), 'paged', 'single', true);
	assert.match(markup, /aria-label="Reading direction"/);
	assert.match(markup, /aria-label="Page turn animation"/);
	assert.match(markup, /aria-label="Chapter transition page"/);
	assert.match(markup, /aria-label="Background"/);
	assert.match(markup, /reader__typography/);
	assert.match(markup, /aria-label="Font"/);
});

test('text-only chapter settings group titles drop the image-specific groups', async () => {
	const markup = await renderSettings(page({ Text: 'A text chapter' }), 'paged', 'single', true);
	const titles = [...markup.matchAll(/reader__settings-title">([^<]+)</g)].map((m) => m[1]);
	assert.deepEqual(titles, ['Reading mode', 'Page turning', 'Appearance', 'Typography']);
});

test('text-only chapters drop the webtoon reading mode', async () => {
	const markup = await renderSettings(page({ Text: 'A text chapter' }), 'paged', 'single', true);
	assert.match(markup, />Paged</);
	assert.match(markup, />Scroll</);
	assert.doesNotMatch(markup, />Webtoon</);
});

test('text chapters keep webtoon out of the mode tiles in scroll mode too', async () => {
	// ReaderPage only sets textOnly in paged mode, so a scrolling text chapter passes textOnly=false
	const markup = await renderSettings(page({ Text: 'A text chapter' }), 'continuous', 'single', false, true);
	assert.match(markup, />Paged</);
	assert.match(markup, />Scroll</);
	assert.doesNotMatch(markup, />Webtoon</);
});

test('paged text columns switch colour on a light canvas', () => {
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8')
		.replace(/\r\n/g, '\n');
	assert.match(css, /\.reader-text--cols \{\n[^}]*color: #e6e6e9;/);
	assert.match(css, /\.reader--bg-light \.reader-text--cols \{\n\tcolor: #1f2937;\n\}/);
});
