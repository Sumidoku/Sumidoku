import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const css = readFileSync(new URL('../src/pages/manga.css', import.meta.url), 'utf8');
const page = readFileSync(new URL('../src/pages/MangaDetailPage.tsx', import.meta.url), 'utf8');

test('tag list wraps instead of scrolling off-screen', () => {
	// A single nowrap row hid every tag past the viewport edge; the row must wrap.
	const rule = css.match(/\.manga-hero__tags\s*\{([^}]*)\}/);
	assert.ok(rule);
	assert.match(rule[1], /flex-wrap:\s*wrap/);
	assert.doesNotMatch(rule[1], /overflow-x/);
});

test('no scroll-hint mask survives on the tag list', () => {
	// The fade mask only made sense for horizontal scrolling; a wrapped list has no edge to hint at.
	const rules = css.replace(/\/\*[\s\S]*?\*\//g, '');
	for (const m of rules.matchAll(/\.manga-hero__tags[^{]*\{([^}]*)\}/g)) {
		assert.doesNotMatch(m[1], /mask-image/);
	}
});

test('the clamp is measured from the third wrapped row, not a fixed pixel count', () => {
	// Clickable chips are 36px tall and plain chips shorter, so a hardcoded clamp crops rows.
	assert.match(page, /rows === 3/);
	assert.match(page, /setTagsClamp\(thirdRowTop >= 0 \? thirdRowTop - TAG_LIST_GAP : null\)/);
	assert.match(page, /maxHeight: tagsClamp/);
});

test('the expand toggle appears only when tags actually overflow two rows', () => {
	assert.match(page, /\{tagsClamp !== null && \(/);
	assert.match(page, /onClick=\{\(\) => setTagsOpen/);
});
