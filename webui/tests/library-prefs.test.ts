import assert from 'node:assert/strict';
import test from 'node:test';
import { hasLibraryFilterPrefs, loadLibraryFilters, saveLibraryFilters } from '../src/lib/libraryFilterPrefs.ts';
import type { LibraryFilterPrefs } from '../src/lib/libraryFilterPrefs.ts';

const KEY = 'sumidoku.library.filters';

function withStorage(run: (values: Map<string, string>) => void) {
	const values = new Map<string, string>();
	const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => values.set(key, String(value))
		}
	});
	try {
		run(values);
	} finally {
		if (original) Object.defineProperty(globalThis, 'localStorage', original);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
}

test('library search, sort and filters round-trip through local storage', () => {
	withStorage(() => {
		const prefs: LibraryFilterPrefs = {
			search: '  二人巴士 ',
			sort: 'latestChapter',
			filters: { unread: 1, bookmarked: -1 },
			ratings: { 2: 1 },
			sources: { 4: -1 },
			categories: { 0: 1 },
			authors: { 'Jane Doe': 1 },
			tags: { romance: -1 },
			group: 7
		};
		saveLibraryFilters(prefs);
		assert.equal(hasLibraryFilterPrefs(), true);
		assert.deepEqual(loadLibraryFilters(), prefs);
	});
});

test('legacy and malformed library prefs get safe defaults', () => {
	withStorage((values) => {
		values.set(KEY, JSON.stringify({ filters: { unread: 1 }, ratings: {}, sources: {}, categories: {}, group: null }));
		assert.deepEqual(loadLibraryFilters(), {
			search: '',
			sort: 'added',
			filters: { unread: 1 },
			ratings: {},
			sources: {},
			categories: {},
			authors: {},
			tags: {},
			group: null
		});
		values.set(KEY, JSON.stringify({ search: 42, sort: 'random', filters: { unread: 0, bookmarked: -1 }, authors: { 'Jane Doe': 2 }, group: '7' }));
		assert.deepEqual(loadLibraryFilters(), {
			search: '',
			sort: 'added',
			filters: { bookmarked: -1 },
			ratings: {},
			sources: {},
			categories: {},
			authors: {},
			tags: {},
			group: null
		});
	});
});
