import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test, { after, before } from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';
import type { QueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import type { RemoteState, TrackerLink } from '../src/api/types.ts';

let vite: ViteDevServer;
let trackers: typeof import('../src/api/trackers.ts');
let createElement: typeof import('react').createElement;
let renderToStaticMarkup: typeof import('react-dom/server').renderToStaticMarkup;
let QueryClient: typeof import('@tanstack/react-query').QueryClient;
let QueryClientProvider: typeof import('@tanstack/react-query').QueryClientProvider;
let MemoryRouter: typeof import('react-router-dom').MemoryRouter;

before(async () => {
	vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	trackers = await vite.ssrLoadModule('/src/api/trackers.ts');
	({ createElement } = await import('react'));
	({ renderToStaticMarkup } = await import('react-dom/server'));
	({ QueryClient, QueryClientProvider } = await import('@tanstack/react-query'));
	({ MemoryRouter } = await import('react-router-dom'));
	const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
	await i18n.changeLanguage('en');
});

after(() => vite.close());

function link(overrides: Partial<TrackerLink> = {}): TrackerLink {
	return {
		mangaId: 7,
		provider: 'anilist',
		trackId: '123',
		title: 'Remote title',
		trackMode: 'auto',
		chapterOffset: 0,
		createdAt: 1,
		updatedAt: 2,
		...overrides
	};
}

function remoteState(): RemoteState {
	return {
		score: 8,
		status: 'reading',
		lastReadChapter: 12,
		lastReadVolume: 2,
		totalChapters: 100
	};
}

function renderWithClient(client: QueryClient, node: ReactNode) {
	return renderToStaticMarkup(createElement(QueryClientProvider, { client }, node));
}

test('tracker pill renders remote status instead of the binding title', async () => {
	const client = new QueryClient();
	client.setQueryData(trackers.trackerKeys.mangaLinks(7), [link()]);
	client.setQueryData(trackers.trackerKeys.state('anilist', '123'), remoteState());
	const { TrackerSection } = await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx');
	const markup = renderWithClient(client, createElement(TrackerSection, { mangaId: 7, mangaTitle: 'Local title' }));
	assert.match(markup, /Reading/);
	assert.doesNotMatch(markup, /Remote title/);
});

test('cover card renders compact tracker provider markers', async () => {
	const { CoverCard } = await vite.ssrLoadModule('/src/components/CoverCard.tsx');
	const markup = renderToStaticMarkup(
		createElement(CoverCard, { title: 'Series', trackerProviders: ['bangumi', 'anilist', 'mal', 'mangabaka'] })
	);
	assert.match(markup, /cover-card__trackers/);
	for (const name of ['Bangumi', 'AniList', 'MyAnimeList', 'MangaBaka']) {
		assert.match(markup, new RegExp(`<img class="tracker-logo[^"]*"[^>]*alt="${name}"`));
	}
	assert.doesNotMatch(markup, />\+\d/);
});

test('tracker edit uses segmented controls instead of native pickers', async () => {
	const { TrackerEditForm } = await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx');
	const markup = renderWithClient(
		new QueryClient(),
		createElement(TrackerEditForm, { link: link(), initialState: remoteState(), onClose: () => {} })
	);
	assert.doesNotMatch(markup, /<select\b/);
	assert.ok((markup.match(/role="tablist"/g) ?? []).length >= 2);
});

test('tracker edit exposes a dedicated remote entry link', async () => {
	const { TrackerEditForm } = await vite.ssrLoadModule('/src/components/trackers/TrackerSection.tsx');
	const markup = renderWithClient(
		new QueryClient(),
		createElement(TrackerEditForm, { link: link(), initialState: remoteState(), onClose: () => {} })
	);
	assert.match(markup, /tracker-edit__remote-head/);
	assert.match(markup, /href="https:\/\/anilist.co\/manga\/123"/);
	assert.match(markup, /Open on AniList/);
});

test('settings groups a manga page under one cover with a provider filter and pager', async () => {
	const client = new QueryClient();
	client.setQueryData(trackers.trackerKeys.bindingsPage(1, undefined), {
		entries: [
			{ ...link({ title: 'AniList remote' }), mangaTitle: 'Local title', mangaCover: 'cover-v2' },
			{
				...link({ provider: 'bangumi', trackId: '456', title: 'Bangumi remote' }),
				mangaTitle: 'Local title',
				mangaCover: 'cover-v2'
			},
			link({ mangaId: 8, provider: 'bangumi', trackId: '789', title: 'Unknown remote' })
		],
		hasNextPage: true,
		totalPages: 3,
		counts: { manga: 45, providers: { anilist: 1, bangumi: 44 } }
	});
	const { TrackersPanel } = await vite.ssrLoadModule('/src/pages/settings/TrackersPanel.tsx');
	let markup = '';
	assert.doesNotThrow(() => {
		markup = renderWithClient(client, createElement(MemoryRouter, null, createElement(TrackersPanel)));
	});
	assert.equal(markup.match(/class="tracker-binding__cover"/g)?.length, 2);
	assert.equal(markup.match(/class="tracker-binding__link"/g)?.length, 3);
	assert.match(markup, /cover-v2/);
	assert.match(markup, /Manga #8/);
	assert.match(markup, /Page 1 of 3/);
	assert.match(markup, /tracker-bindings__filter/);
	assert.doesNotMatch(markup, /MangaBaka<\/span><span class="tracker-seg__count"/);
});
