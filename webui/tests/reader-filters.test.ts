import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';
import type { Page } from '../src/api/types.ts';
import {
	BLEND_MODES,
	buildFilterString,
	buildTintStyle,
	DEFAULT_FILTERS,
	FILTER_LIMITS,
	hasActiveFilters,
	parseFilters
} from '../src/pages/reader/filters.ts';
import type { ReaderFilters } from '../src/pages/reader/filters.ts';
import { DEFAULT_TYPOGRAPHY } from '../src/pages/reader/typography.ts';

function page(content: Page['content']): Page {
	return { content, thumbnail: null, hasDescription: false, description: null, index: 0 };
}

async function renderSettingsWithFilters(filters: ReaderFilters, textOnly = false): Promise<string> {
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
					onModeChange: () => {},
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
					currentPage: page({ Url: ['https://example.invalid/page.png', null] }),
					textOnly,
					typography: DEFAULT_TYPOGRAPHY,
					onTypographyChange: () => {},
					filters,
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

test('parseFilters reads back a serialized default', () => {
	assert.deepEqual(parseFilters(JSON.stringify(DEFAULT_FILTERS)), DEFAULT_FILTERS);
});

test('parseFilters rejects missing, malformed and non-object values', () => {
	assert.equal(parseFilters(null), null);
	assert.equal(parseFilters(''), null);
	assert.equal(parseFilters('{oops'), null);
	assert.equal(parseFilters('42'), null);
	assert.equal(parseFilters('"dark"'), null);
	assert.equal(parseFilters('[]'), null);
});

test('parseFilters fills missing fields with defaults and ignores unknown keys', () => {
	const parsed = parseFilters(JSON.stringify({ sepia: true, nightMode: true }));
	assert.deepEqual(parsed, { ...DEFAULT_FILTERS, sepia: true });
});

test('parseFilters clamps slider values into their limits', () => {
	const parsed = parseFilters(
		JSON.stringify({
			brightness: { enabled: true, value: 999 },
			contrast: { enabled: true, value: 0 },
			hue: { enabled: true, value: 720 },
			rgba: { enabled: true, red: 300, green: -5, blue: 128.4, alpha: 250, blendMode: 'screen' }
		})
	);
	assert.ok(parsed);
	assert.equal(parsed.brightness.value, FILTER_LIMITS.brightness.max);
	assert.equal(parsed.contrast.value, FILTER_LIMITS.contrast.min);
	assert.equal(parsed.hue.value, FILTER_LIMITS.hue.max);
	assert.equal(parsed.rgba.red, 255);
	assert.equal(parsed.rgba.green, 0);
	assert.equal(parsed.rgba.blue, 128);
	assert.equal(parsed.rgba.alpha, 100);
	assert.equal(parsed.rgba.blendMode, 'screen');
});

test('parseFilters rejects malformed entries instead of guessing', () => {
	assert.equal(parseFilters(JSON.stringify({ brightness: { enabled: 'yes', value: 100 } })), null);
	assert.equal(parseFilters(JSON.stringify({ sepia: 1 })), null);
	assert.equal(
		parseFilters(
			JSON.stringify({
				rgba: { enabled: true, red: 0, green: 0, blue: 0, alpha: 50, blendMode: 'color-dodge' }
			})
		),
		null
	);
});

test('every advertised blend mode round-trips', () => {
	for (const blendMode of BLEND_MODES) {
		const parsed = parseFilters(JSON.stringify({ rgba: { ...DEFAULT_FILTERS.rgba, blendMode } }));
		assert.equal(parsed?.rgba.blendMode, blendMode);
	}
});

test('buildFilterString is empty when every filter is off', () => {
	assert.equal(buildFilterString(DEFAULT_FILTERS), '');
});

test('buildFilterString combines enabled filters in a stable order, excluding rgba', () => {
	const filters: ReaderFilters = {
		...DEFAULT_FILTERS,
		brightness: { enabled: true, value: 120 },
		contrast: { enabled: true, value: 90 },
		saturate: { enabled: true, value: 150 },
		hue: { enabled: true, value: 45 },
		sepia: true,
		grayscale: true,
		invert: true,
		rgba: { enabled: true, red: 255, green: 0, blue: 0, alpha: 50, blendMode: 'multiply' }
	};
	assert.equal(
		buildFilterString(filters),
		'brightness(1.2) contrast(0.9) saturate(1.5) hue-rotate(45deg) invert(1) sepia(1) grayscale(1)'
	);
});

test('buildTintStyle is null until the rgba filter is enabled', () => {
	assert.equal(buildTintStyle(DEFAULT_FILTERS), null);
	const style = buildTintStyle({
		...DEFAULT_FILTERS,
		rgba: { enabled: true, red: 255, green: 0, blue: 128, alpha: 25, blendMode: 'overlay' }
	});
	assert.deepEqual(style, { background: 'rgba(255, 0, 128, 0.25)', mixBlendMode: 'overlay' });
});

test('hasActiveFilters tracks the enabled flags only', () => {
	assert.equal(hasActiveFilters(DEFAULT_FILTERS), false);
	assert.equal(hasActiveFilters({ ...DEFAULT_FILTERS, brightness: { enabled: true, value: 100 } }), true);
	assert.equal(hasActiveFilters({ ...DEFAULT_FILTERS, invert: true }), true);
	assert.equal(hasActiveFilters({ ...DEFAULT_FILTERS, rgba: { ...DEFAULT_FILTERS.rgba, enabled: true } }), true);
});

test('the filters group renders collapsed switches and a disabled reset by default', async () => {
	const markup = await renderSettingsWithFilters(DEFAULT_FILTERS);
	assert.match(markup, /reader__settings-title">Filters</);
	assert.doesNotMatch(markup, /reader__filter-slider/);
	assert.match(markup, /disabled="">Reset</);
});

test('enabling a level filter reveals its slider', async () => {
	const markup = await renderSettingsWithFilters({
		...DEFAULT_FILTERS,
		brightness: { enabled: true, value: 120 }
	});
	assert.match(markup, /aria-label="Brightness"[^>]*value="120"/);
	assert.match(markup, /reader__filter-slider/);
	assert.match(markup, /<output>120%<\/output>/);
	assert.doesNotMatch(markup, /disabled="">Reset</);
});

test('enabling the tint reveals channel sliders and the blend-mode combobox', async () => {
	const markup = await renderSettingsWithFilters({
		...DEFAULT_FILTERS,
		rgba: { enabled: true, red: 255, green: 128, blue: 0, alpha: 25, blendMode: 'multiply' }
	});
	for (const channel of ['Red', 'Green', 'Blue', 'Opacity']) {
		assert.match(markup, new RegExp(`aria-label="${channel}"`));
	}
	assert.match(markup, /role="combobox" aria-label="Blend mode"/);
	assert.match(markup, /<span class="select__value">Multiply<\/span>/);
});

test('text-only chapters hide the filters group', async () => {
	const markup = await renderSettingsWithFilters(DEFAULT_FILTERS, true);
	assert.doesNotMatch(markup, /reader__settings-title">Filters</);
});

test('the reader root carries the filter variable, the tint overlay and the storage keys', () => {
	const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /'--reader-filter': filterString/);
	assert.match(source, /buildTintStyle\(filters\)/);
	// The tint must stay unmounted in text-only chapters: the controls are hidden
	// there, so a stored tint would otherwise cover the text with no way to turn it off
	assert.match(source, /\{!textOnly && tintStyle !== null && <div className="reader__tint"/);
	assert.match(source, /'sumidoku\.reader\.filters'/);
	assert.match(source, /sumidoku\.reader\.filters\.\$\{mangaId\}/);
});

test('the stylesheet applies the variable to page images and pins the tint below the chrome', () => {
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
	assert.match(css, /\.reader__img,\n\.reader__pg > img \{\n\tfilter: var\(--reader-filter, none\);\n\}/);
	assert.match(css, /\.reader__tint \{\n\tposition: fixed;\n\tinset: 0;\n\tz-index: 2;\n\tpointer-events: none;\n\}/);
});
