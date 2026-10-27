import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const css = readFileSync(new URL('../src/components/components.css', import.meta.url), 'utf8');

test('segmented items center their content instead of sitting on the text baseline', () => {
	// Icon labels (the reader mode switch) are inline-flex boxes; inside a plain button
	// they align to the baseline and float above the thumb's center.
	const item = css.match(/\.seg__item\s*\{([^}]*)\}/)?.[1];
	assert.ok(item);
	assert.match(item, /\bdisplay:\s*inline-flex;/);
	assert.match(item, /\balign-items:\s*center;/);
	assert.match(item, /\bjustify-content:\s*center;/);
	// Without the text line box an icon-only item would shrink below a text segment.
	assert.match(item, /\bmin-height:\s*calc\(1lh \+ 10px\);/);
});
