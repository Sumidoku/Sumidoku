import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const browse = readFileSync(new URL('../src/pages/SourceBrowsePage.tsx', import.meta.url), 'utf8');
const listingLabels = readFileSync(new URL('../src/lib/localListings.ts', import.meta.url), 'utf8');

test('OPDS browse keeps Search as the default and exposes All plus catalog navigation listings', () => {
	assert.match(browse, /useState<string>\(\(\) => searchParams\.get\('mode'\) \?\? 'search'\)/);
	assert.match(browse, /useSourceListings\(sourceId, sourceEnabled\)/);
	assert.match(browse, /options=\{\[\s*\{ value: 'search', label: t\('common\.search'\) \},\s*\.\.\.listings\.map/);
	assert.match(browse, /function switchMode\(m: string\)/);
	assert.match(browse, /value: l\.id/);
	assert.match(browse, /label: isLocal \|\| isOpds \? localListingLabel\(l\.id, l\.name, t\) : l\.name/);
	assert.match(browse, /<InfiniteList[\s\S]*?browseGroups\.map/);
});

test('OPDS empty Search does not request the root feed implicitly', () => {
	assert.match(browse, /const isEmptyOpdsSearch = isOpds && isSearch && submitted\.query === ''/);
	assert.match(browse, /searchOptions\.enabled && !isEmptyOpdsSearch/);
});

test('OPDS All listing uses the shared translated All label', () => {
	assert.match(listingLabels, /'opds:all': 'sourceHome\.all'/);
});

test('remote search submits explicitly and OPDS fallback is visible', () => {
	assert.match(browse, /!isLocal && \(\s*<Button type="submit"/);
	assert.match(browse, /searchQuery\.data\?\.pages\.some\(\(page\) => page\.degraded\)/);
	assert.match(browse, /sourceBrowse\.degradedSearch/);
});

test('OPDS browse renders publications in the cover grid and groups in independent home scrollers', () => {
	assert.match(browse, /<div className="cover-grid">[\s\S]*?entries\.map/);
	assert.match(browse, /browseGroups\.map\(\(group, groupIndex\)/);
	assert.match(browse, /activeQuery\.data\?\.pages\.flatMap\(\(page\) => page\.groups \?\? \[\]\)/);
	assert.match(browse, /<div className="home-hrow">[\s\S]*?group\.entries\.map/);
	assert.match(browse, /onClick=\{\(\) => void openEntry\(m\)\}/);
	assert.match(browse, /entries\.length === 0 && browseGroups\.length === 0 && browseNavigation\.length === 0 && \(!isOpds \|\| !activeQuery\.hasNextPage\)/);
});

test('OPDS navigation appears in feed and group sections and follows listing IDs with a parent link', () => {
	assert.match(browse, /page\.navigation \?\? \[\]/);
	assert.match(browse, /\(group\.navigation \?\? \[\]\)\.map\(\(item\)/);
	assert.match(browse, /onClick=\{\(\) => switchMode\(item\.listingId\)\}/);
	assert.match(browse, /parentListingId &&/);
	assert.match(browse, /switchMode\(parentListingId\)/);
});
