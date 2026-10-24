import assert from 'node:assert/strict';
import test from 'node:test';
import { applyDownloadEvent, mergeDownloadResponse } from '../src/lib/downloadEvents.ts';
import type { DownloadChapterEvent } from '../src/api/protocol.ts';
import type { Download, DownloadStatus } from '../src/api/types.ts';

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

function finished(chapterId: number): DownloadChapterEvent {
	return { type: 'finished', chapterId, downloaded: 9, total: 10 };
}

test('stale full responses cannot overwrite a finished download', () => {
	const event = finished(7);
	const current = applyDownloadEvent([download(7, 'downloading')], event);
	const response = [{ ...download(7, 'downloading'), downloaded: 2 }];

	const merged = mergeDownloadResponse(current, response, new Map([[7, event]]));

	assert.equal(merged[0].status, 'finished');
	assert.equal(merged[0].downloaded, 10);
	assert.equal(merged[0].total, 10);
	assert.equal(merged[0].error, null);
});

test('unseen response rows replay the latest cached finished event', () => {
	const events = new Map<number, DownloadChapterEvent>();
	events.set(7, { type: 'started', chapterId: 7, downloaded: 0, total: 10 });
	events.set(7, finished(7));

	const merged = mergeDownloadResponse([], [download(7, 'queued')], events);

	assert.equal(merged[0].status, 'finished');
	assert.equal(merged[0].downloaded, 10);
	assert.equal(merged[0].total, 10);
	assert.equal(merged[0].mangaTitle, 'Manga');
});
