import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

test('library toolbar stays collapsed with active filters and summarizes them', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { default: LibraryPage } = await vite.ssrLoadModule('/src/pages/LibraryPage.tsx');
		const { LibraryToolbar } = await vite.ssrLoadModule('/src/pages/library/LibraryToolbar.tsx');
		const pageMarkup = renderToStaticMarkup(
			createElement(
				QueryClientProvider,
				{ client: new QueryClient() },
				createElement(MemoryRouter, null, createElement(LibraryPage))
			)
		);
		assert.match(pageMarkup, /class="library__toolbar"/);
		assert.match(pageMarkup, /class="library__categories"/);
		assert.match(pageMarkup, /aria-label="Library controls"/);
		assert.match(pageMarkup, /aria-label="Sort"/);
		assert.match(pageMarkup, /aria-expanded="false"/);
		assert.doesNotMatch(pageMarkup, /class="library__filter-panel"/);

		const activeMarkup = renderToStaticMarkup(
			createElement(LibraryToolbar, {
				searchInput: '',
				onSearchInput: () => {},
				sort: 'added',
				onSort: () => {},
				filterTri: { unread: 1 },
				ratings: {},
				sourceTri: {},
				categoryTri: { '3': 1 },
				authorTri: { 'Jane Doe': 1 },
				tagTri: {},
				groupId: 7,
				groups: [{ id: 7, name: 'Unread only', sortOrder: 0, rules: [] }],
				categories: [{ id: 3, name: 'Reading', mangaCount: 12, isDefault: false, sortOrder: 0 }],
				sourceOptions: [],
				authorOptions: [{ key: 'Jane Doe', label: 'Jane Doe', count: 4 }],
				tagOptions: [{ key: 'romance', label: 'romance', count: 9 }],
				showUncategorized: true,
				onFilterCycle: () => {},
				onRatingCycle: () => {},
				onSourceCycle: () => {},
				onCategoryCycle: () => {},
				onAuthorCycle: () => {},
				onTagCycle: () => {},
				onGroupChange: () => {},
				onRemoveFilter: () => {},
				onResetRatings: () => {},
				onResetSources: () => {},
				onResetAuthors: () => {},
				onResetTags: () => {},
				onClearFilters: () => {}
			})
		);
		assert.match(activeMarkup, /aria-expanded="false"/);
		assert.doesNotMatch(activeMarkup, /class="library__filter-panel"/);
		assert.match(activeMarkup, /class="library__filter-count">3</);
		assert.match(activeMarkup, /Reading<span class="library__category-count">12<\/span>/);
		assert.doesNotMatch(activeMarkup, /Reading[（(]12[）)]/);
		assert.match(activeMarkup, /aria-label="Active filters"/);
		assert.match(activeMarkup, /class="library__active-label">Unread</);
		assert.match(activeMarkup, /class="library__active-label">Author: Jane Doe</);
		assert.match(activeMarkup, /class="library__active-label">Group: Unread only</);
		assert.match(activeMarkup, />Clear filters</);
	} finally {
		await vite.close();
	}
});

test('active filter summary names every advanced filter with its state', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { activeFilterItems, advancedFilterCount } = await vite.ssrLoadModule('/src/pages/library/LibraryToolbar.tsx');
		const selections = {
			filterTri: { unread: 1, downloaded: -1 },
			ratings: { '3': -1 },
			sourceTri: { '5': 1, '6': 1 },
			authorTri: { 'Jane Doe': -1 },
			tagTri: { romance: 1 },
			groupId: 7,
			groups: [{ id: 7, name: 'Weekend', sortOrder: 0, rules: [] }],
			// Counts in the dropdown label stay out of the pill; source 6 has not loaded yet
			sourceOptions: [{ key: '5', label: 'Alpha', count: 12, name: 'Alpha' }]
		};
		const items = activeFilterItems(i18n.t, selections);
		assert.equal(activeFilterItems(i18n.t, {
			...selections,
			filterTri: { completed: 1 }
		})[0].label, 'Completed');
		assert.deepEqual(
			items.map(({ label, state, kind, filterKey }: { label: string; state: number; kind: string | null; filterKey: string }) => [label, state, kind, filterKey]),
			[
				['Unread', 1, 'filter', 'unread'],
				['Downloaded', -1, 'filter', 'downloaded'],
				['Rating: NSFW', -1, 'rating', '3'],
				['Source: Alpha', 1, 'source', '5'],
				['Source', 1, 'source', '6'],
				['Author: Jane Doe', -1, 'author', 'Jane Doe'],
				['Tag: romance', 1, 'tag', 'romance'],
				['Group: Weekend', 1, null, '']
			]
		);
		assert.equal(
			items.length,
			advancedFilterCount(selections.filterTri, selections.ratings, selections.sourceTri, selections.authorTri, selections.tagTri, selections.groupId)
		);
	} finally {
		await vite.close();
	}
});

test('tri-state chips keep the off state compact and use distinct active icons', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { Chip } = await vite.ssrLoadModule('/src/components/Chip.tsx');
		const off = renderToStaticMarkup(createElement(Chip, { triState: 0 }, 'Uncategorized'));
		const included = renderToStaticMarkup(createElement(Chip, { triState: 1 }, 'Uncategorized'));
		const excluded = renderToStaticMarkup(createElement(Chip, { triState: -1 }, 'Uncategorized'));
		assert.deepEqual(
			{
				offHasMarker: off.includes('chip__tri-state'),
				includedMarkerBeforeLabel: included.indexOf('chip__tri-state') < included.indexOf('>Uncategorized<'),
				excludedMarkerBeforeLabel: excluded.indexOf('chip__tri-state') < excluded.indexOf('>Uncategorized<'),
				excludedHasBan: excluded.includes('lucide-ban'),
				excludedHasMinus: excluded.includes('lucide-minus')
			},
			{
				offHasMarker: false,
				includedMarkerBeforeLabel: true,
				excludedMarkerBeforeLabel: true,
				excludedHasBan: true,
				excludedHasMinus: false
			}
		);
	} finally {
		await vite.close();
	}
});

test('advanced filter count excludes search, categories and sorting', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { advancedFilterCount } = await vite.ssrLoadModule('/src/pages/library/LibraryToolbar.tsx');
		assert.equal(advancedFilterCount({}, {}, {}, {}, {}, null), 0);
		assert.equal(advancedFilterCount({ unread: 1, downloaded: -1 }, { '1': 1 }, { '2': 1, '3': -1 }, {}, {}, 7), 6);
		assert.equal(advancedFilterCount({}, {}, {}, { 'Jane Doe': 1 }, { romance: -1 }, null), 2);
	} finally {
		await vite.close();
	}
});
