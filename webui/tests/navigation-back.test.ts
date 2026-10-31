import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { hasHistoryBack } from '../src/lib/navigation.ts';

// Entered from another in-app page (e.g. an author/tag tap on a manga detail pushes
// /browse/:sourceId?filters=...), so back must pop that entry, not push a fixed page
test('an idx above zero means an in-app previous entry exists', () => {
	assert.equal(hasHistoryBack(1), true);
	assert.equal(hasHistoryBack(3), true);
});

// idx 0 (fresh load) or a missing state (deep link) has no in-app back target:
// navigate(-1) there would leave the app, so callers use their fixed fallback
test('direct entry has no in-app back target', () => {
	assert.equal(hasHistoryBack(0), false);
	assert.equal(hasHistoryBack(undefined), false);
});

test('source browse back pops history instead of always pushing the source list', () => {
	const source = readFileSync(new URL('../src/pages/SourceBrowsePage.tsx', import.meta.url), 'utf8');
	// Both back buttons (the disabled-source branch and the normal header) go through history
	assert.doesNotMatch(source, /onClick=\{\(\) => navigate\('\/browse'\)\}/);
	assert.match(
		source,
		/if \(hasHistoryBack\(window\.history\.state\?\.idx\)\) navigate\(-1\);\s*else navigate\('\/browse'\);/
	);
});

test('source home backs pop history instead of pushing a fixed page', () => {
	const source = readFileSync(new URL('../src/pages/SourceHomePage.tsx', import.meta.url), 'utf8');
	// A pushed parent destination on a back button ping-pongs forever with the browse
	// page's pop-back; the forward search/goSearch buttons keep their fixed paths
	assert.doesNotMatch(source, /onClick=\{\(\) => navigate\('\/browse'\)\}/);
	assert.match(source, /label=\{t\('sourceHome\.backToBrowse'\)\} onClick=\{goBack\}/);
	assert.match(source, /onClick=\{goBackToSource\}/);
	assert.match(source, /else navigate\(`\/browse\/\$\{sourceId\}`\);/);
});

test('detail and reader backs share the same history check', () => {
	for (const page of ['MangaDetailPage.tsx', 'ReaderPage.tsx']) {
		const source = readFileSync(new URL(`../src/pages/${page}`, import.meta.url), 'utf8');
		assert.match(source, /hasHistoryBack\(window\.history\.state\?\.idx\)/, page);
	}
});
