import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// A text chapter is a single page, so "last spread" holds for the whole
// chapter: without these gates the swipe-to-turn drag every reader tries on a
// text page would also arm the pull-to-next-chapter, and a release past the
// threshold would commit it mid-reading. The gates keep the pull out of text
// pages while the swipe itself turns their columns
const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('the paged pull tracks whether the current page is text', () => {
	assert.match(source, /textPageRef\.current = curText !== null;/);
});

test('the paged drag never feeds the pull on a text page', () => {
	// The pull spot is the last spread of an image page — or, with the
	// transition page on, the parked transition page itself
	assert.match(source, /\(transitionRef\.current \? pendingTurnRef\.current === 'next' : !textPageRef\.current\)/);
	assert.match(source, /if \(pullEnabledRef\.current && atPullSpot\) \{/);
});

test('turning onto a text page abandons an active pull', () => {
	assert.match(
		source,
		/\(!atLastSpreadRef\.current \|\| textPageRef\.current\) && pullCtlRef\.current && pullCtlRef\.current\.rawPull > 0\) pullCtlRef\.current\.cancel\(\);/
	);
});

test('the paged hint strip is not rendered for a text page', () => {
	// Nor with the transition page on: the pull's progress lives in the page
	assert.match(source, /\{navEnabled && nextCh !== null && curText === null && !transition && \(/);
});

test('with the transition page on, a pull away from the page only parks on it', () => {
	assert.match(source, /if \(mode === 'paged' && transition && pendingTurn !== 'next'\) \{/);
});

test('the transition page rides the track on a virtual slot past either chapter end', () => {
	assert.match(source, /'\-\-transition\-slot': -1/);
	assert.match(source, /'\-\-transition\-slot': spreads\.length/);
});

// Fade turns make the track a block with absolutely stacked spreads; the hint
// stays a flex row item only in slide mode, so it needs its own placement there
const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('fade turns park the paged hint past the trailing edge instead of in flow', () => {
	assert.match(css, /\.reader__track--fade \.reader__pull-p \{\n\tposition: absolute;\n\tinset-block: 0;\n\tinset-inline-start: 100%;/);
});
