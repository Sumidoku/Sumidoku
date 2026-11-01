import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { createServer } from 'vite';
import type { Category, Manga, Source } from '../src/api/types.ts';

const CATEGORIES: Category[] = [
	{ id: 3, name: 'Reading', sortOrder: 0, isDefault: false, mangaCount: 2 },
	{ id: 4, name: 'Later', sortOrder: 1, isDefault: false, mangaCount: 1 }
];
const SOURCE: Source = {
	id: 1,
	key: 'test',
	name: 'Test source',
	kind: 'aidoku',
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
const MANGA: Manga = {
	id: 1,
	sourceId: 1,
	key: 'manga',
	title: 'Test manga',
	cover: null,
	artists: null,
	authors: null,
	description: null,
	url: null,
	tags: ['romance'],
	status: 'Completed',
	content_rating: 'Safe',
	viewer: 'RightToLeft',
	update_strategy: 'Always',
	next_update_time: null,
	chapters: null,
	initialized: true,
	trashed: false,
	chapterCount: 0,
	inLibrary: true,
	incognito: false,
	categoryIds: [4],
	trackerProviders: []
};

test('manga detail shows only its assigned categories before the tags', async () => {
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
		const { qk } = await vite.ssrLoadModule('/src/api/hooks.ts');
		const { default: MangaDetailPage } = await vite.ssrLoadModule('/src/pages/MangaDetailPage.tsx');
		const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
		qc.setQueryData(qk.manga(1), MANGA);
		qc.setQueryData(qk.chapters(1), []);
		qc.setQueryData(qk.categories, CATEGORIES);
		qc.setQueryData(qk.sources, [SOURCE]);
		qc.setQueryData(qk.sourceFilters(1), []);
		const markup = renderToStaticMarkup(
			createElement(
				QueryClientProvider,
				{ client: qc },
				createElement(MemoryRouter, { initialEntries: ['/manga/1'] },
					createElement(Routes, null, createElement(Route, { path: '/manga/:id', element: createElement(MangaDetailPage) }))
				)
			)
		);
		const categories = markup.match(/<div class="manga-hero__categories">(.*?)<\/div>/)?.[1];
		assert.ok(categories);
		assert.match(categories, /class="chip">Later</);
		assert.doesNotMatch(categories, /Reading/);
		assert.doesNotMatch(categories, /mangaCount|manga-count|[（(]\d+[）)]/);
		assert.ok(markup.indexOf('manga-hero__categories') < markup.indexOf('manga-hero__tags'));
	} finally {
		await vite.close();
	}
});
