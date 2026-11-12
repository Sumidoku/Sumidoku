import assert from 'node:assert/strict';
import test from 'node:test';
import {
	crossfadeEnabled,
	fadeProgress,
	FADE_TRAVEL_PX,
	parsePageTurn,
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

test('a turn holds the outgoing page until the target is ready, with a bounded timeout', () => {
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
