import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { createServer } from 'vite';

function chapter(isRead: boolean) {
	return {
		id: 1,
		key: '1',
		title: null,
		chapter_number: 1,
		volume_number: null,
		date_uploaded: null,
		scanlators: null,
		url: null,
		language: null,
		thumbnail: null,
		locked: false,
		pageCount: 10,
		lastPageRead: 0,
		isRead,
		isBookmarked: false,
		lastReadAt: null,
		isDownloaded: false
	};
}

async function renderChapter(isRead: boolean) {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		await vite.ssrLoadModule('/src/i18n');
		const { ChapterRow } = await vite.ssrLoadModule('/src/pages/manga/ChapterRow.tsx');
		return renderToStaticMarkup(
			createElement(
				MemoryRouter,
				null,
				createElement(ChapterRow, {
					chapter: chapter(isRead),
					selecting: false,
					selected: false,
					onActivate: () => {},
					dlState: 'idle',
					onDownload: () => {},
					onToggleBookmark: () => {},
					bookmarkPending: false
				})
			)
		);
	} finally {
		await vite.close();
	}
}

test('unread marker appears only on unread chapter rows', async () => {
	const unread = await renderChapter(false);
	assert.match(unread, /class="chapter-row__unread"/);
	assert.match(unread, />Unread</);

	const read = await renderChapter(true);
	assert.doesNotMatch(read, /chapter-row__unread/);
	assert.doesNotMatch(read, />Unread</);
});
