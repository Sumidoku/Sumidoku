import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
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
		const { MenuButton } = await vite.ssrLoadModule('/src/components/MenuButton.tsx');
		const menu = renderToStaticMarkup(createElement(MenuButton, { label: 'Actions', disabled: true }, () => null));
		assert.match(menu, /aria-label="Actions"[^>]*disabled/);
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
