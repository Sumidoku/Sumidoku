import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { createMangaDownloadEventHandlers, downloadActivitySnapshot } from '../src/lib/downloadEvents.ts';
import type { MangaDownloadActivity } from '../src/lib/downloadEvents.ts';
import type { DownloadChapterEvent } from '../src/api/protocol.ts';
import type { ChapterWithDownload, Download, DownloadStatus } from '../src/api/types.ts';

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

function download(chapterId: number, status: DownloadStatus): Download {
	return {
		chapterId,
		mangaId: 3,
		mangaTitle: 'Manga',
		chapterTitle: `Chapter ${chapterId}`,
		chapterNumber: chapterId,
		volumeNumber: null,
		status,
		format: 'cbz',
		path: null,
		error: status === 'error' ? 'failed' : null,
		retries: 0,
		downloaded: status === 'downloading' ? 1 : null,
		total: status === 'downloading' ? 10 : null,
		createdAt: 1,
		updatedAt: 1
	};
}

function harness(initial: ChapterWithDownload[] | undefined) {
	let chapters = initial;
	let activity = new Map<number, MangaDownloadActivity>();
	const invalidated: Array<readonly unknown[]> = [];
	const handlers = createMangaDownloadEventHandlers(chaptersKey, libraryKey, {
		setChapters: (updater) => {
			chapters = updater(chapters);
		},
		invalidate: (queryKey) => invalidated.push(queryKey),
		setActivity: (chapterId, value) => {
			const next = new Map(activity);
			if (value) next.set(chapterId, value);
			else next.delete(chapterId);
			activity = next;
		},
		syncActivity: (downloads) => {
			activity = downloadActivitySnapshot(downloads, 3);
		}
	});
	return { handlers, invalidated, getActivity: () => activity, getChapters: () => chapters };
}

test('finished updates cached chapters and invalidates chapters and library', () => {
	const h = harness([chapter(7), chapter(8)]);

	h.handlers.onDownload({ type: 'started', chapterId: 7, downloaded: 0, total: 10 });
	h.handlers.onDownload(finished);

	const chapters = h.getChapters();
	assert.ok(chapters);
	assert.equal(chapters[0].isDownloaded, true);
	assert.equal(chapters[1].isDownloaded, false);
	assert.deepEqual(h.invalidated, [chaptersKey, libraryKey]);
	assert.equal(h.getActivity().has(7), false);
});

test('started and progress expose page counts until the terminal event', () => {
	const h = harness(undefined);

	h.handlers.onDownload({ type: 'started', chapterId: 7, downloaded: 0, total: 10 });
	assert.deepEqual(h.getActivity().get(7), {
		status: 'downloading',
		progress: { downloaded: 0, total: 10 }
	});
	h.handlers.onDownload({ type: 'progress', chapterId: 7, downloaded: 4, total: 10 });
	assert.deepEqual(h.getActivity().get(7), {
		status: 'downloading',
		progress: { downloaded: 4, total: 10 }
	});
	h.handlers.onDownload({ type: 'error', chapterId: 7, downloaded: 4, total: 10 });
	assert.equal(h.getActivity().has(7), false);
});

test('finished and snapshot invalidate chapter queries that have no cached data', () => {
	const h = harness(undefined);

	h.handlers.onDownload(finished);
	assert.equal(h.getChapters(), undefined);
	assert.deepEqual(h.invalidated, [chaptersKey, libraryKey]);

	h.invalidated.length = 0;
	h.handlers.onSnapshot([]);
	assert.deepEqual(h.invalidated, [chaptersKey]);
});

test('snapshot clears terminal chapters without clearing active downloads', () => {
	const h = harness(undefined);

	h.handlers.onSnapshot([download(7, 'error'), download(8, 'finished'), download(9, 'downloading')]);

	assert.deepEqual(h.invalidated, [chaptersKey]);
	assert.deepEqual(h.getActivity(), new Map([
		[9, { status: 'downloading', progress: { downloaded: 1, total: 10 } }]
	]));
});

test('manga detail wires subscriptions, seeds activity and cleans them up', () => {
	const source = readFileSync(new URL('../src/pages/MangaDetailPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /createMangaDownloadEventHandlers\(qk\.chapters\(mangaId\), qk\.library/);
	assert.match(source, /sse\.on\('download'/);
	assert.match(source, /sse\.on\('snapshot', \(msg\) =>/);
	assert.match(source, /handlers\.onSnapshot\(\(msg as EventSnapshot\)\.downloads\)/);
	assert.match(source, /api<Download\[\]>\('\/downloads'\)/);
	assert.match(source, /if \(active && !receivedEvent\) handlers\.onSnapshot\(downloads\)/);
	assert.match(source, /unsubDownload\(\);\s*unsubSnapshot\(\);/);
	assert.doesNotMatch(source, /setDlQueued|subscribeWS/);
});
