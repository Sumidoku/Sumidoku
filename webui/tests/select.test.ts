import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test, { after, before } from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import { moveIndex, placeMenu, selectedIndex, typeaheadIndex } from '../src/components/selectModel.ts';

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
