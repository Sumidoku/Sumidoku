import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
	crossfadeEnabled,
	fadeProgress,
	FADE_TRAVEL_PX,
	parsePageTurn,
	shouldAnimateSpread,
	shouldRevealTurn,
	TURN_HOLD_FADE_MS,
	TURN_HOLD_MS
} from '../src/pages/reader/pageTurn.ts';

test('only the fade style crossfades: it needs animations, no split page and the style', () => {
	assert.equal(crossfadeEnabled('fade', true, false), true);
	assert.equal(crossfadeEnabled('slide', true, false), false);
	// The animations switch turns every style into an instant swap
	assert.equal(crossfadeEnabled('fade', false, false), false);
	// A split page changes its crop on the outgoing image and cannot wait
	assert.equal(crossfadeEnabled('fade', true, true), false);
});

test('a turn holds the outgoing page until the target is ready, with delayed loading feedback', () => {
	assert.equal(shouldRevealTurn(0, 1, false, false), false);
	assert.equal(shouldRevealTurn(0, 1, true, false), true);
	assert.equal(shouldRevealTurn(0, 0, false, false), true);
	assert.equal(shouldRevealTurn(0, 0, true, false), true);
	assert.equal(shouldRevealTurn(0, 1, false, true), true);
	assert.equal(TURN_HOLD_MS, 500);
	// A crossfade paints the target underneath, so it holds for less
	assert.equal(TURN_HOLD_FADE_MS, 250);
	assert.ok(TURN_HOLD_FADE_MS < TURN_HOLD_MS);
});

const readerSource = readFileSync(new URL('../src/pages/ReaderPage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

test('the opening page aligns the track before its image loads', () => {
	// A resume/deep link must not leave the track on spread zero while its real
	// opening page loads.
	assert.ok(/setIdx\(initial\);[\s\S]*?setShownSpread\(spreadIndexOf\(spreads \?\? \[\], initial\)\);/.test(readerSource));
});

test('an unfinished target never appears just because the hold timer expired', () => {
	// The hold timer may surface a spinner, but revealing on timeout would show
	// the very blank/loading target the outgoing page is meant to cover.
	assert.doesNotMatch(readerSource, /setTimeout\(\(\) => setShownSpread\(/);
});

test('a fade target is fully opaque underneath while only the outgoing page fades', () => {
	const css = readFileSync(new URL('../src/pages/reader.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
	const rule = css.match(/\.reader__track--fade \.reader__spread--current \{([^}]*)\}/)?.[1] ?? '';
	assert.match(rule, /opacity: 1;/);
	// Fading the target in as well lets the black canvas show through the
	// middle of the turn: two half-transparent pages do not make an opaque one.
	assert.match(rule, /transition: none;/);
});

test('an instant jump restores gesture transitions after its position paints', () => {
	assert.ok(
		/requestAnimationFrame\(\(\) => trackRef\.current\?\.classList\.remove\('reader__track--instant'\)\)/.test(
			readerSource
		)
	);
});

test('a decoded automatic retry reports readiness under the stable page URL', () => {
	const source = readFileSync(new URL('../src/pages/reader/PageImage.tsx', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
	// spreadReady waits for the base page URL, not a particular retry attempt.
	assert.doesNotMatch(source, /readyCallbackRef\.current\?\.\(effectiveSrc\)/);
	assert.match(source, /readyCallbackRef\.current\?\.\(src\)/);
});

test('only an adjacent spread change animates the sliding track', () => {
	assert.equal(shouldAnimateSpread(false, 0, 1), false, 'the opening alignment is not a turn');
	assert.equal(shouldAnimateSpread(true, 0, 5), false, 'a slider jump must not sweep through intermediate pages');
	assert.equal(shouldAnimateSpread(true, 4, 5), true);
	assert.equal(shouldAnimateSpread(true, 5, 4), true);
	assert.equal(
		shouldAnimateSpread(true, 5, 5),
		true,
		'same-position renders must keep boundary and spring-back transitions'
	);
});

test('stored turn styles parse against the supported values, and old auto slides', () => {
	assert.equal(parsePageTurn('slide'), 'slide');
	assert.equal(parsePageTurn('fade'), 'fade');
	// Stored before the style split: landscape used to fade, the rest slid
	assert.equal(parsePageTurn('auto'), 'slide');
	assert.equal(parsePageTurn('instant'), null);
	assert.equal(parsePageTurn(null), null);
});

test('a drag drives the crossfade toward its direction, clamped to its capacity', () => {
	assert.equal(fadeProgress(0, 'next', 1), 0);
	assert.equal(fadeProgress(FADE_TRAVEL_PX / 2, 'next', 1), 0.5);
	assert.equal(fadeProgress(FADE_TRAVEL_PX, 'next', 1), 1);
	// Past the full travel the outgoing page stays gone
	assert.equal(fadeProgress(FADE_TRAVEL_PX * 3, 'next', 1), 1);
	// Backward drags read their own sign
	assert.equal(fadeProgress(-FADE_TRAVEL_PX / 2, 'prev', 1), 0.5);
	assert.equal(fadeProgress(-FADE_TRAVEL_PX / 2, 'next', 1), 0);
	assert.equal(fadeProgress(FADE_TRAVEL_PX / 2, 'prev', 1), 0);
	// Dragging the wrong way never goes negative
	assert.equal(fadeProgress(-100, 'next', 1), 0);
	// An unloaded target caps the fade instead of showing a blank spread
	assert.equal(fadeProgress(FADE_TRAVEL_PX, 'next', 0.25), 0.25);
	// No committed direction means no fade at all
	assert.equal(fadeProgress(FADE_TRAVEL_PX, null, 1), 0);
});
