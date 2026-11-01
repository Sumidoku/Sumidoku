import assert from 'node:assert/strict';
import test from 'node:test';
import { crossfadeEnabled, parsePageTurn, shouldRevealTurn, TURN_HOLD_MS } from '../src/pages/reader/pageTurn.ts';

test('automatic turns fade only in landscape and respect split pages and animation opt-out', () => {
	assert.equal(crossfadeEnabled('auto', true, true, false), true);
	assert.equal(crossfadeEnabled('auto', false, true, false), false);
	assert.equal(crossfadeEnabled('slide', true, true, false), false);
	assert.equal(crossfadeEnabled('fade', false, true, false), true);
	assert.equal(crossfadeEnabled('fade', true, false, false), false);
	assert.equal(crossfadeEnabled('auto', true, true, true), false);
});

test('a turn holds the outgoing page until the target is ready, with a bounded timeout', () => {
	assert.equal(shouldRevealTurn(0, 1, false, false), false);
	assert.equal(shouldRevealTurn(0, 1, true, false), true);
	assert.equal(shouldRevealTurn(0, 0, false, false), true);
	assert.equal(shouldRevealTurn(0, 0, true, false), true);
	assert.equal(shouldRevealTurn(0, 1, false, true), true);
	assert.equal(TURN_HOLD_MS, 500);
});

test('stored turn styles parse against the supported values', () => {
	assert.equal(parsePageTurn('auto'), 'auto');
	assert.equal(parsePageTurn('slide'), 'slide');
	assert.equal(parsePageTurn('fade'), 'fade');
	assert.equal(parsePageTurn('instant'), null);
	assert.equal(parsePageTurn(null), null);
});
