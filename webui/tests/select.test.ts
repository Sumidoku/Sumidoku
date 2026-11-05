import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test, { after, before } from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import { moveIndex, placeMenu, placeMenuVertical, selectedIndex, typeaheadIndex } from '../src/components/selectModel.ts';

const options = [
	{ value: 'title', label: 'Title' },
	{ value: 'recent', label: 'Recently read' },
	{ value: 'rating', label: 'Rating' },
	{ value: 'added', label: 'Added' }
];

let vite: ViteDevServer;
let createElement: typeof import('react').createElement;
let renderToStaticMarkup: typeof import('react-dom/server').renderToStaticMarkup;

before(async () => {
	vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	({ createElement } = await import('react'));
	({ renderToStaticMarkup } = await import('react-dom/server'));
});

after(async () => {
	await vite.close();
});

test('an unknown value shows the first option, as a native select does', () => {
	assert.equal(selectedIndex(options, 'rating'), 2);
	assert.equal(selectedIndex(options, 'gone'), 0);
	assert.equal(selectedIndex([], ''), -1);
});

test('navigation keys stop at the list ends and ignore other keys', () => {
	assert.equal(moveIndex('ArrowDown', 0, 4), 1);
	assert.equal(moveIndex('ArrowDown', 3, 4), 3);
	assert.equal(moveIndex('ArrowUp', 0, 4), 0);
	assert.equal(moveIndex('Home', 2, 4), 0);
	assert.equal(moveIndex('End', 1, 4), 3);
	assert.equal(moveIndex('PageDown', 1, 4), null);
});

test('typeahead matches a prefix, cycles on a repeated letter and wraps', () => {
	assert.equal(typeaheadIndex(options, 'a', 0), 3);
	assert.equal(typeaheadIndex(options, 'r', 0), 1);
	assert.equal(typeaheadIndex(options, 'rr', 1), 2);
	assert.equal(typeaheadIndex(options, 'r', 2), 1);
	assert.equal(typeaheadIndex(options, 'ra', 1), 2);
	assert.equal(typeaheadIndex(options, 'z', 0), -1);
});

const viewport = { width: 400, height: 800 };

test('the menu opens below the anchor when it fits', () => {
	const place = placeMenu({ top: 100, bottom: 136, left: 20 }, { width: 200, height: 200 }, viewport);
	assert.deepEqual(place, { top: 140, left: 20, maxHeight: 260 });
});

test('the menu flips above an anchor near the bottom edge', () => {
	const place = placeMenu({ top: 700, bottom: 736, left: 20 }, { width: 200, height: 200 }, viewport);
	assert.deepEqual(place, { top: 496, left: 20, maxHeight: 260 });
});

test('the menu keeps inside the viewport at the right edge and on short screens', () => {
	const right = placeMenu({ top: 100, bottom: 136, left: 300 }, { width: 200, height: 100 }, viewport);
	assert.equal(right.left, 192);
	const short = placeMenu({ top: 40, bottom: 76, left: 0 }, { width: 200, height: 200 }, { width: 400, height: 200 });
	assert.equal(short.maxHeight, 112);
	assert.equal(short.top, 80);
});

test('a bottom-preferred menu opens below its anchor while it fits', () => {
	assert.deepEqual(placeMenuVertical({ top: 100, bottom: 136 }, 200, 800), { top: 140, maxHeight: 260 });
});

test('a bottom-preferred menu flips above an anchor near the bottom edge', () => {
	// #619: the last source row's ⋮ on a phone — 220px of menu against 52px below the anchor
	assert.deepEqual(placeMenuVertical({ top: 700, bottom: 736 }, 220, 800), { top: 476, maxHeight: 260 });
});

test('a bottom-preferred menu stays down when neither side fits and below has more room', () => {
	assert.deepEqual(placeMenuVertical({ top: 200, bottom: 236 }, 700, 800), { top: 240, maxHeight: 260 });
});

test('a top-preferred menu opens above its anchor while it fits', () => {
	assert.deepEqual(placeMenuVertical({ top: 700, bottom: 736 }, 200, 800, 'top'), { top: 496, maxHeight: 260 });
});

test('a top-preferred menu flips below when only that side has room', () => {
	// The batch action bar at the top of a short list: 28px above the anchor, the menu needs 200
	assert.deepEqual(placeMenuVertical({ top: 40, bottom: 76 }, 200, 800, 'top'), { top: 80, maxHeight: 260 });
});

test('a menu taller than the room on its side is clamped so it can scroll', () => {
	assert.deepEqual(placeMenuVertical({ top: 40, bottom: 76 }, 200, 200), { top: 80, maxHeight: 112 });
});

test('a closed Select renders a labelled combobox and no native picker', async () => {
	const { Select } = await vite.ssrLoadModule('/src/components/Select.tsx');
	const markup = renderToStaticMarkup(
		createElement(
			Select,
			{ value: 2, onChange: () => {}, 'aria-label': 'Interval', disabled: true },
			createElement('option', { value: 1 }, 'Hourly'),
			[createElement('option', { key: 2, value: 2 }, 'Every ', 2, ' hours')]
		)
	);
	assert.doesNotMatch(markup, /<select\b/);
	assert.match(markup, /role="combobox"/);
	assert.match(markup, /aria-label="Interval"/);
	assert.match(markup, /aria-expanded="false"/);
	assert.match(markup, /disabled=""/);
	assert.match(markup, /<span class="select__value">Every 2 hours<\/span>/);
	// Both labels stay in the sizer so the width holds whichever value is shown
	assert.equal(markup.match(/select__sizer/g)?.length, 2);
});

test('action menus are limited by the screen, not the listbox cap', () => {
	// A ten-row language menu is about 366px: it must fit whole when the screen has room
	assert.deepEqual(placeMenuVertical({ top: 100, bottom: 136 }, 366, 900, 'bottom', Infinity), { top: 140, maxHeight: 752 });
	// Near the bottom edge it still flips up and clamps to the room above
	assert.deepEqual(placeMenuVertical({ top: 600, bottom: 636 }, 366, 700, 'bottom', Infinity), { top: 230, maxHeight: 588 });
});

test('MenuButton positions without the listbox cap', () => {
	const source = readFileSync(new URL('../src/components/MenuButton.tsx', import.meta.url), 'utf8');
	assert.match(source, /placeMenuVertical\(anchor, pop\.height, window\.innerHeight, placement, Infinity\)/);
});

test('closing an anchored menu drops its measured position', () => {
	// A stale maxHeight would cap the next measurement and misplace the reopened menu
	const source = readFileSync(new URL('../src/components/MenuButton.tsx', import.meta.url), 'utf8');
	assert.match(source, /if \(!open\) \{\s*setPos\(null\);\s*return;\s*\}/);
});
