import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { QueryClient } from '@tanstack/react-query';
import { createServer } from 'vite';

test('page image URLs include width and the chapter cache revision', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { pageUrl } = await vite.ssrLoadModule('/src/api/client.ts');
		const url = new URL(pageUrl(19, 3, 1200, 8), 'http://sumidoku.test');
		assert.equal(url.pathname, '/api/v1/chapter/19/page/3');
		assert.equal(url.searchParams.get('width'), '1200');
		assert.equal(url.searchParams.get('revision'), '8');
	} finally {
		await vite.close();
	}
});

test('chapter pages become stale immediately so reopening refetches within the global stale window', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { chapterPagesQueryOptions } = await vite.ssrLoadModule('/src/api/hooks.ts');
		const queryClient = new QueryClient({ defaultOptions: { queries: { staleTime: 30_000 } } });
		let requests = 0;
		const options = chapterPagesQueryOptions(44);
		const queryFn = async () => ++requests;
		await queryClient.fetchQuery({ ...options, queryFn });
		await queryClient.fetchQuery({ ...options, queryFn });
		assert.equal(requests, 2);
		assert.equal(options.refetchOnMount, 'always');
		queryClient.clear();
	} finally {
		await vite.close();
	}
});

test('finished downloads expose chapter actions, while a disabled menu remains keyboard-disabled', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
		const { MenuButton, isMenuPopupVisible } = await vite.ssrLoadModule('/src/components/MenuButton.tsx');
		const menu = renderToStaticMarkup(createElement(MenuButton, { label: 'Actions', disabled: true }, () => null));
		assert.match(menu, /aria-label="Actions"[^>]*disabled/);
		assert.equal(isMenuPopupVisible(true, true), false);
		assert.equal(isMenuPopupVisible(true, false), true);
		const { RemoveFinishedDownloadAction } = await vite.ssrLoadModule('/src/pages/manga/ChapterRow.tsx');
		const removeAction = renderToStaticMarkup(createElement(RemoveFinishedDownloadAction, { available: true, onRemove: () => {} }));
		const noRemoveAction = renderToStaticMarkup(createElement(RemoveFinishedDownloadAction, { available: false, onRemove: () => {} }));
		assert.match(removeAction, />Remove download</);
		assert.doesNotMatch(noRemoveAction, /Remove download/);
		const { ChapterRow } = await vite.ssrLoadModule('/src/pages/manga/ChapterRow.tsx');
		const chapter = {
			id: 12, key: '12', title: null, chapter_number: 12, volume_number: null,
			date_uploaded: null, scanlators: null, url: null, language: null,
			thumbnail: null, locked: false, pageCount: 5, lastPageRead: 0,
			isRead: false, isBookmarked: false, lastReadAt: null,
			isDownloaded: true, hasFinishedDownload: true, trashed: false
		};
		const row = renderToStaticMarkup(createElement(MemoryRouter, null, createElement(ChapterRow, {
			chapter, selecting: false, selected: false, onActivate: () => {}, dlState: 'idle',
			onDownload: () => {}, onToggleBookmark: () => {}, bookmarkPending: false,
			onRemoveDownload: () => {}
		})));
		assert.match(row, /aria-label="Chapter actions"/);
	} finally {
		await vite.close();
	}
});
