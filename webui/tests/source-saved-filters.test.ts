import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import type { FilterValue } from '../src/api/types.ts';
import {
	addSavedFilter,
	loadSavedFilters,
	removeSavedFilter,
	saveSavedFilters
} from '../src/lib/sourceSavedFilters.ts';

const KEY = (sourceId: number) => `sumidoku.sourceSavedFilters.${sourceId}`;

const filters: FilterValue[] = [
	{ Text: { id: 'author', value: 'alice' } },
	{ MultiSelect: { id: 'genre', included: ['action'], excluded: [] } }
];

function withStorage(run: (values: Map<string, string>) => void) {
	const values = new Map<string, string>();
	const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => values.set(key, String(value)),
			removeItem: (key: string) => values.delete(key)
		}
	});
	try {
		run(values);
	} finally {
		if (original) Object.defineProperty(globalThis, 'localStorage', original);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
}

test('saved filter groups round-trip per source with their query and filters', () => {
	withStorage(() => {
		const result = addSavedFilter(1, ' 周更动作 ', ' 修仙 ', filters);
		assert.equal(result.ok, true);
		assert.deepEqual(result.groups, [{ name: '周更动作', query: '修仙', filters }]);
		assert.deepEqual(loadSavedFilters(1), [{ name: '周更动作', query: '修仙', filters }]);
		// another source sees none of it
		assert.deepEqual(loadSavedFilters(2), []);
		assert.deepEqual(addSavedFilter(2, 'second', null, []).groups, [{ name: 'second', query: null, filters: [] }]);
	});
});

test('new groups append in creation order', () => {
	withStorage(() => {
		addSavedFilter(1, 'one', null, []);
		const result = addSavedFilter(1, 'two', 'q', filters);
		assert.equal(result.ok, true);
		assert.deepEqual(
			loadSavedFilters(1).map((g) => g.name),
			['one', 'two']
		);
	});
});

test('duplicate and empty names are refused without touching the store', () => {
	withStorage(() => {
		addSavedFilter(1, 'list', null, filters);
		const duplicate = addSavedFilter(1, '  list ', null, []);
		assert.equal(duplicate.ok, false);
		assert.equal(duplicate.ok ? '' : duplicate.reason, 'duplicate');
		assert.deepEqual(duplicate.groups, [{ name: 'list', query: null, filters }]);
		const empty = addSavedFilter(1, '   ', null, []);
		assert.equal(empty.ok, false);
		assert.equal(empty.ok ? '' : empty.reason, 'empty');
		assert.deepEqual(loadSavedFilters(1), [{ name: 'list', query: null, filters }]);
	});
});

test('an empty query collapses to null when saved', () => {
	withStorage(() => {
		const result = addSavedFilter(1, 'blank-query', '   ', filters);
		assert.equal(result.ok, true);
		assert.deepEqual(loadSavedFilters(1), [{ name: 'blank-query', query: null, filters }]);
	});
});

test('removing the last group clears the storage key', () => {
	withStorage((values) => {
		addSavedFilter(1, 'only', null, filters);
		assert.ok(values.has(KEY(1)));
		assert.deepEqual(removeSavedFilter(1, 'only'), []);
		assert.equal(values.has(KEY(1)), false);
		assert.deepEqual(loadSavedFilters(1), []);
	});
});

test('broken JSON and malformed entries degrade to what is still usable', () => {
	withStorage((values) => {
		values.set(KEY(1), '{not json');
		assert.deepEqual(loadSavedFilters(1), []);
		values.set(
			KEY(1),
			JSON.stringify([
				{ name: '  ', query: 'x', filters: [] },
				{ name: 42, filters: [] },
				'not an object',
				{ name: 'kept', query: 'q', filters: [filters[0], null, 'bad'] },
				{ name: 'kept', query: 'dup', filters: [] }
			])
		);
		assert.deepEqual(loadSavedFilters(1), [{ name: 'kept', query: 'q', filters: [filters[0]] }]);
	});
});

test('saving an empty list removes the key instead of keeping []', () => {
	withStorage((values) => {
		addSavedFilter(1, 'gone', null, []);
		saveSavedFilters(1, []);
		assert.equal(values.has(KEY(1)), false);
	});
});

test('the browse page applies saved groups through the shared filter restore path', () => {
	const source = readFileSync(new URL('../src/pages/SourceBrowsePage.tsx', import.meta.url), 'utf8');
	assert.match(source, /applyFilterValues\(states, group\.filters\)/);
	assert.match(source, /runSearch\(nextStates, group\.query \?\? ''\)/);
	assert.match(source, /removeSavedFilter\(sourceId, g\.name\)/);
	assert.match(source, /addSavedFilter\(sourceId, name, input\.trim\(\), buildFilterValues\(states\)\)/);
});
