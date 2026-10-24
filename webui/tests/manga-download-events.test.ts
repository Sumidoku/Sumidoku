import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createMangaDownloadEventHandlers } from '../src/lib/downloadEvents.ts';
import type { DownloadChapterEvent } from '../src/api/protocol.ts';
import type { ChapterWithDownload } from '../src/api/types.ts';

const chaptersKey = ['manga', 3, 'chapters'] as const;
const libraryKey = ['library'] as const;
const finished: DownloadChapterEvent = { type: 'finished', chapterId: 7, downloaded: 10, total: 10 };

function chapter(id: number): ChapterWithDownload {
	return {
		id,
		key: String(id),
		title: null,
		chapter_number: id,
		volume_number: null,
		date_uploaded: null,
		scanlators: null,
		url: null,
		language: null,
		thumbnail: null,
		locked: false,
		pageCount: 10,
		lastPageRead: 0,
		isRead: false,
		isBookmarked: false,
		lastReadAt: null,
		isDownloaded: false
	};
}

function harness(initial: ChapterWithDownload[] | undefined) {
	let chapters = initial;
	const invalidated: Array<readonly unknown[]> = [];
	const cleared: number[] = [];
	const handlers = createMangaDownloadEventHandlers(chaptersKey, libraryKey, {
		setChapters: (updater) => {
			chapters = updater(chapters);
		},
		invalidate: (queryKey) => invalidated.push(queryKey),
		clearQueued: (chapterId) => cleared.push(chapterId)
	});
	return { handlers, invalidated, cleared, getChapters: () => chapters };
}

test('finished updates cached chapters and invalidates chapters and library', () => {
	const h = harness([chapter(7), chapter(8)]);

	h.handlers.onDownload(finished);

	const chapters = h.getChapters();
	assert.ok(chapters);
	assert.equal(chapters[0].isDownloaded, true);
	assert.equal(chapters[1].isDownloaded, false);
	assert.deepEqual(h.invalidated, [chaptersKey, libraryKey]);
	assert.deepEqual(h.cleared, [7]);
});

test('finished and snapshot invalidate chapter queries that have no cached data', () => {
	const h = harness(undefined);

	h.handlers.onDownload(finished);
	assert.equal(h.getChapters(), undefined);
	assert.deepEqual(h.invalidated, [chaptersKey, libraryKey]);

	h.invalidated.length = 0;
	h.handlers.onSnapshot();
	assert.deepEqual(h.invalidated, [chaptersKey]);
});

test('manga detail wires both subscriptions and cleans them up', () => {
	const source = readFileSync(new URL('../src/pages/MangaDetailPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /createMangaDownloadEventHandlers\(qk\.chapters\(mangaId\), qk\.library/);
	assert.match(source, /sse\.on\('download'/);
	assert.match(source, /sse\.on\('snapshot', handlers\.onSnapshot\)/);
	assert.match(source, /unsubDownload\(\);\s*unsubSnapshot\(\);/);
	assert.doesNotMatch(source, /subscribeWS/);
});
