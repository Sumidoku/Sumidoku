import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import { downloadEligibleIds } from '../src/lib/librarySelection.ts';
import type { Category, LibraryEntry, Source } from '../src/api/types.ts';

function entry(id: number, sourceId: number, title: string, overrides: Partial<LibraryEntry> = {}): LibraryEntry {
	return {
		id,
		sourceId,
		key: String(id),
		title,
		cover: null,
		artists: null,
		authors: null,
		description: null,
		url: null,
		tags: null,
		status: 'Ongoing',
		content_rating: 'Safe',
		viewer: 'RightToLeft',
		update_strategy: 'Always',
		next_update_time: null,
		chapters: null,
		initialized: true,
		trashed: false,
		chapterCount: 12,
		inLibrary: true,
		incognito: false,
		categoryIds: [],
		inLibraryAt: 100,
		unreadCount: 0,
		...overrides
	};
}

function source(id: number, kind: Source['kind'], name: string): Source {
	return {
		id,
		key: String(id),
		name,
		kind,
		version: 1,
		languages: ['en'],
		contentRating: 1,
		baseUrl: null,
		breakingChangeVersion: 0,
		config: null,
		enabled: true,
		installedAt: 1,
		updatedAt: 1,
		updateVersion: null,
		missingFromRepos: false,
		hasHome: null,
		pinned: false,
		pinOrder: null
	};
}

const READING: Category = { id: 3, name: 'Reading', sortOrder: 0, isDefault: false, mangaCount: 2 };
const FITNESS: Category = { id: 4, name: 'Fitness', sortOrder: 1, isDefault: false, mangaCount: 1 };
const SOURCES: Source[] = [source(1, 'aidoku', 'Source A'), source(2, 'local', 'Local files')];

/** Alpha is downloadable, Beta is local-source, Gamma is trashed */
const ENTRIES: LibraryEntry[] = [
	entry(10, 1, 'Alpha', { unreadCount: 2 }),
	entry(11, 2, 'Beta'),
	entry(12, 1, 'Gamma', { trashed: true })
];

async function renderLibrary(
	vite: ViteDevServer,
	opts: { entries?: LibraryEntry[]; initialSelecting?: boolean } = {}
) {
	const { entries = ENTRIES, initialSelecting = false } = opts;
	// the assertions match English labels: force the language instead of inheriting the machine locale
	const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
	await i18n.changeLanguage('en');
	const { qk } = await vite.ssrLoadModule('/src/api/hooks.ts');
	const { default: LibraryPage } = await vite.ssrLoadModule('/src/pages/LibraryPage.tsx');
	const qc = new QueryClient();
	// Mirrors the page's default browse state (empty search/filters, 'added' sort)
	const browse = {
		search: undefined,
		sort: 'added',
		filters: {},
		sourceId: undefined,
		ratings: {},
		sources: {},
		categories: {},
		authors: {},
		tags: {},
		groupId: undefined
	};
	qc.setQueryData([...qk.library, browse], {
		pages: [{ entries, hasNextPage: false }],
		pageParams: [1]
	});
	qc.setQueryData(qk.sources, SOURCES);
	qc.setQueryData(qk.categories, [READING, FITNESS]);
	return renderToStaticMarkup(
		createElement(
			QueryClientProvider,
			{ client: qc },
			createElement(MemoryRouter, null, createElement(LibraryPage, { initialSelecting }))
		)
	);
}

async function withVite<T>(fn: (vite: ViteDevServer) => Promise<T>): Promise<T> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		return await fn(vite);
	} finally {
		await vite.close();
	}
}

test('normal mode keeps the toolbar and offers Select in the header', async () => {
	const markup = await withVite((vite) => renderLibrary(vite));
	assert.match(markup, /class="library__toolbar"/);
	assert.match(markup, />Select</);
	assert.doesNotMatch(markup, /batch-bar/);
	assert.match(markup, /href="\/manga\/10"/);
});

test('selection mode hides the toolbar, auto-selects the loaded entries and shows the batch bar', async () => {
	const markup = await withVite((vite) => renderLibrary(vite, { initialSelecting: true }));
	assert.doesNotMatch(markup, /class="library__toolbar"/);
	// All three loaded entries start selected, so the header offers deselect
	assert.match(markup, />Deselect all</);
	assert.match(markup, />3 manga selected</);
	assert.match(markup, />Done</);
	assert.match(markup, /class="batch-bar"/);
	assert.match(markup, />3 selected</);
	// Every card is a pressed toggle button, and no card navigates
	assert.equal([...markup.matchAll(/aria-pressed="true"/g)].length, 3);
	assert.doesNotMatch(markup, /href="\/manga\//);
	assert.match(markup, /cover-card__check/);
	// Beta (local) and Gamma (trashed) are skipped, so the hint explains the reduced download scope
	assert.match(markup, /title="Local-source and trashed entries are skipped">Download</);
	assert.doesNotMatch(markup, /disabled="" title="Local-source and trashed entries are skipped"/);
	for (const label of ['Delete downloads', 'Mark read', 'Mark unread', 'Change categories', 'Remove from library']) {
		assert.match(markup, new RegExp(`>${label}<`));
	}
});

test('download is disabled when every selected entry is local or trashed', async () => {
	const markup = await withVite((vite) =>
		renderLibrary(vite, {
			entries: [entry(11, 2, 'Beta'), entry(12, 1, 'Gamma', { trashed: true })],
			initialSelecting: true
		})
	);
	assert.match(markup, /disabled="" title="Local-source and trashed entries are skipped">Download</);
});

test('downloadEligibleIds skips local-source and trashed entries', () => {
	const ids = downloadEligibleIds(
		[
			{ id: 10, sourceId: 1, trashed: false },
			{ id: 11, sourceId: 2, trashed: false },
			{ id: 12, sourceId: 1, trashed: true }
		],
		new Set([2])
	);
	assert.deepEqual(ids, [10]);
});

test('category dialog replaces the categories of every selected entry and starts blank', async () => {
	await withVite(async (vite) => {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { ChangeCategoriesDialog } = await vite.ssrLoadModule('/src/pages/library/LibraryBatchModals.tsx');
		const markup = renderToStaticMarkup(
			createElement(ChangeCategoriesDialog, {
				count: 3,
				categories: [READING, FITNESS],
				busy: false,
				onCancel: () => {},
				onApply: () => {}
			})
		);
		assert.match(markup, /replace the current categories of all 3 selected manga/);
		assert.match(markup, /aria-label="Reading"/);
		assert.match(markup, /aria-label="Fitness"/);
		// Replace semantics: nothing is prechecked, the final set is whatever the user picks
		assert.doesNotMatch(markup, /checked/);
		assert.match(markup, />Save</);
		assert.match(markup, />Cancel</);

		const empty = renderToStaticMarkup(
			createElement(ChangeCategoriesDialog, {
				count: 3,
				categories: [],
				busy: false,
				onCancel: () => {},
				onApply: () => {}
			})
		);
		assert.match(empty, /No categories yet/);
	});
});

test('remove from library asks for confirmation with the selected count', async () => {
	await withVite(async (vite) => {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		await i18n.changeLanguage('en');
		const { RemoveFromLibraryDialog } = await vite.ssrLoadModule('/src/pages/library/LibraryBatchModals.tsx');
		const markup = renderToStaticMarkup(
			createElement(RemoveFromLibraryDialog, {
				count: 2,
				busy: false,
				onCancel: () => {},
				onConfirm: () => {}
			})
		);
		assert.match(markup, /Remove 2 selected manga from the library\?/);
		assert.match(markup, /class="btn btn--danger"[^>]*>Remove</);
		assert.match(markup, />Cancel</);
	});
});
