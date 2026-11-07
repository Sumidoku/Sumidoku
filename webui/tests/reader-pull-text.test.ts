import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// Paged mode has no swipe navigation — taps, keys and wheel turn pages, and a
// text page's columns the same way (turn() advances columns before any chapter
// boundary). A horizontal drag is still the gesture a reader tries on a text
// page, and a text chapter is a single page, so "last spread" holds for the
// whole chapter: without these gates that natural drag would arm the
// pull-to-next-chapter and a release would commit it mid-reading
const source = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('the paged pull tracks whether the current page is text', () => {
	assert.match(source, /textPageRef\.current = curText !== null;/);
});

test('the paged drag never feeds the pull on a text page', () => {
	assert.match(source, /ctl\.feedTouch\(dx \* forwardSignRef\.current, atLastSpreadRef\.current && !textPageRef\.current\);/);
});

test('turning onto a text page abandons an active pull', () => {
	assert.match(
		source,
		/\(!atLastSpreadRef\.current \|\| textPageRef\.current\) && pullCtlRef\.current && pullCtlRef\.current\.rawPull > 0\) pullCtlRef\.current\.cancel\(\);/
	);
});

test('the paged hint strip is not rendered for a text page', () => {
	assert.match(source, /\{navEnabled && nextCh !== null && curText === null && \(/);
});

// Fade turns make the track a block with absolutely stacked spreads; the hint
// stays a flex row item only in slide mode, so it needs its own placement there
const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('fade turns park the paged hint past the trailing edge instead of in flow', () => {
	assert.match(css, /\.reader__track--fade \.reader__pull-p \{\n\tposition: absolute;\n\tinset-block: 0;\n\tinset-inline-start: 100%;/);
});
