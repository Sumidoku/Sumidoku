import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

test('library toolbar groups controls and reveals active advanced filters', async () => {
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
				groupId: 7,
				groups: [{ id: 7, name: 'Unread only', sortOrder: 0, rules: [] }],
				categories: [{ id: 3, name: 'Reading', mangaCount: 12, isDefault: false, sortOrder: 0 }],
				sourceOptions: [],
				showUncategorized: true,
				onFilterCycle: () => {},
				onRatingCycle: () => {},
				onSourceCycle: () => {},
				onCategoryCycle: () => {},
				onGroupChange: () => {},
				onResetRatings: () => {},
				onResetSources: () => {},
				onClearFilters: () => {}
			})
		);
		assert.match(activeMarkup, /aria-expanded="true"/);
		assert.match(activeMarkup, /class="library__filter-count">2</);
		assert.match(activeMarkup, /class="library__filter-panel"/);
		assert.match(activeMarkup, />Status</);
		assert.match(activeMarkup, />More filters</);
		assert.match(activeMarkup, />Filter groups</);
		assert.match(activeMarkup, /Reading（12）/);
		assert.match(activeMarkup, />Clear filters</);
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
		assert.equal(advancedFilterCount({}, {}, {}, null), 0);
		assert.equal(advancedFilterCount({ unread: 1, downloaded: -1 }, { '1': 1 }, { '2': 1, '3': -1 }, 7), 6);
	} finally {
		await vite.close();
	}
});
