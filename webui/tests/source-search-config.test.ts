import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import type { FilterValue } from '../src/api/types.ts';
import { sourceSearchOptions } from '../src/lib/sourceSearch.ts';

const stored: FilterValue[] = [{ Text: { id: 'author', value: 'alice' } }];
const deepLinked: FilterValue[] = [{ Select: { id: 'genre', value: 'action' } }];

test('source browse uses the config-aware request options', () => {
	const source = readFileSync(new URL('../src/pages/SourceBrowsePage.tsx', import.meta.url), 'utf8');
	assert.match(source, /sourceSearchOptions\(sourceConfig, submitted, isSearch, sourceEnabled\)/);
	assert.match(source, /useSourceSearch\(\s*sourceId,\s*searchOptions.request,\s*searchOptions.enabled/);
});

test('restored searches wait for the source configuration', () => {
	const options = sourceSearchOptions(undefined, { query: 'foo', filters: stored }, true, true);
	assert.equal(options.enabled, false);
	assert.deepEqual(options.request, { query: 'foo', filters: stored });
});

test('hidden-filter sources submit no filters once configuration is ready', () => {
	const options = sourceSearchOptions({ hidesFiltersWhileSearching: true }, { query: 'foo', filters: stored }, true, true);
	assert.equal(options.enabled, true);
	assert.deepEqual(options.request, { query: 'foo', filters: [] });
});

test('normal sources apply stored filters after the same readiness gate', () => {
	const pending = sourceSearchOptions(undefined, { query: 'foo', filters: stored }, true, true);
	assert.equal(pending.enabled, false);
	const ready = sourceSearchOptions({}, { query: 'foo', filters: stored }, true, true);
	assert.equal(ready.enabled, true);
	assert.deepEqual(ready.request, { query: 'foo', filters: stored });
});

test('deep-link filters keep their values and precedence after readiness', () => {
	const pending = sourceSearchOptions(undefined, { query: 'foo', filters: deepLinked }, true, true);
	assert.equal(pending.enabled, false);
	const ready = sourceSearchOptions({}, { query: 'foo', filters: deepLinked }, true, true);
	assert.equal(ready.enabled, true);
	assert.deepEqual(ready.request, { query: 'foo', filters: deepLinked });
});

test('hidden-filter sources only drop filters for a non-empty query', () => {
	const options = sourceSearchOptions({ hidesFiltersWhileSearching: true }, { query: '', filters: stored }, true, true);
	assert.deepEqual(options.request, { query: undefined, filters: stored });
});
