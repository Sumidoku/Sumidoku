// The swipe-to-turn decision is what makes mid-chapter drags feel honest:
// cross half the viewport (or flick hard enough with a little travel) and the
// release turns the page, anything less springs back. These pins cover both
// reading directions, since screen signs flip with RTL.
import test from 'node:test';
import assert from 'node:assert/strict';

import { decideSwipe } from '../src/pages/reader/swipeTurn.ts';

test('crossing half the viewport turns the page in that direction (LTR)', () => {
	// LTR: forward is a leftward (negative) drag
	assert.equal(decideSwipe(-500, 0, 1000, -1), 'forward');
	assert.equal(decideSwipe(500, 0, 1000, -1), 'backward');
});

test('crossing half the viewport turns the page in that direction (RTL)', () => {
	// RTL: forward is a rightward (positive) drag
	assert.equal(decideSwipe(500, 0, 1000, 1), 'forward');
	assert.equal(decideSwipe(-500, 0, 1000, 1), 'backward');
});

test('just short of the midpoint springs back', () => {
	assert.equal(decideSwipe(-499, 0, 1000, -1), null);
	assert.equal(decideSwipe(499, 0, 1000, 1), null);
});

test('a fast flick turns with little travel, a slow drag with the same travel does not', () => {
	assert.equal(decideSwipe(-30, -0.6, 1000, -1), 'forward');
	assert.equal(decideSwipe(-30, -0.4, 1000, -1), null);
});

test('a flick still needs minimum travel, and only counts in its own direction', () => {
	assert.equal(decideSwipe(-20, -0.6, 1000, -1), null);
	// Fast but backward: not a forward turn
	assert.equal(decideSwipe(30, 0.6, 1000, -1), 'backward');
	assert.equal(decideSwipe(-30, 0.6, 1000, -1), null);
});

test('degenerate inputs never turn', () => {
	assert.equal(decideSwipe(0, 0, 1000, -1), null);
	assert.equal(decideSwipe(-500, 0, 0, -1), null);
	assert.equal(decideSwipe(-500, 0, -100, -1), null);
});

// Wiring pins: the swipe decision only matters while the release routes
// through the same turn() the tap zones use, and the text column strip is the
// follow target on text pages
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const textColumns = readFileSync(new URL('../src/pages/reader/TextColumns.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('a swipe release turns through the shared turn path', () => {
	assert.match(source, /decideSwipe\(totalDx, velocity, root\.clientWidth, forwardSignRef\.current\)/);
	assert.match(source, /turnRef\.current\(decision === 'forward'\);/);
});

test('the column strip follows the finger through its own variable and transition cut', () => {
	assert.match(source, /cols\.classList\.add\('reader__swiping'\);/);
	assert.match(textColumns, /var\(--text-swipe, 0px\)/);
	assert.match(css, /\.reader__paged\[data-anim='true'\] \.reader-text--cols\.reader__swiping \{\n\ttransition: none;/);
});

test('a pull that owns the drag keeps the swipe out of the shared variable', () => {
	assert.match(source, /if \(ctl\.feedTouch\(dx \* forwardSignRef\.current, true\)\) \{\n\t\t\t\t\tpullOwned = true;/);
});
