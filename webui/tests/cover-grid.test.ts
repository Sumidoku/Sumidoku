import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

function columns(viewportWidth: number, minCardWidth: number): number {
	const contentWidth = viewportWidth - 32;
	return Math.floor((contentWidth + 12) / (minCardWidth + 12));
}

test('small tablets use larger cover cards instead of phone-density grids', () => {
	const css = readFileSync(new URL('../src/pages/pages.css', import.meta.url), 'utf8');
	assert.match(css, /@media \(min-width: 600px\) and \(max-width: 767px\)\s*\{\s*\.cover-grid\s*\{[^}]*grid-template-columns:\s*repeat\(auto-fill, minmax\(140px, 1fr\)\);/);
	assert.equal(columns(744, 140), 4, 'iPad mini portrait fits four covers');
	assert.equal(columns(390, 104), 3, 'phones keep their existing density');
});
