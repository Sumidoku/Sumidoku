import assert from 'node:assert/strict';
import test from 'node:test';
import {
	applyDownloadEvent,
	applyRetrySuccess,
	downloadActivitySnapshot,
	formatDownloadProgress,
	mergeDownloadResponse
} from '../src/lib/downloadEvents.ts';
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

test('error responses keep their latest error text while merging state', () => {
	const event: DownloadChapterEvent = { type: 'error', chapterId: 7, downloaded: 1, total: 10 };
	const current = applyDownloadEvent([{ ...download(7, 'downloading'), error: 'old error' }], event);
	const response = [{ ...download(7, 'error'), error: 'new error' }];

	const merged = mergeDownloadResponse(current, response, new Map([[7, event]]));

	assert.equal(merged[0].status, 'error');
	assert.equal(merged[0].error, 'new error');
});

test('download progress text includes page counts and a percentage', () => {
	assert.equal(formatDownloadProgress({ downloaded: 3, total: 8 }), '3 / 8 · 37%');
	assert.equal(formatDownloadProgress({ downloaded: 2, total: 0 }), '2');
});

test('retry success re-queues only rows still errored', () => {
	const rows = [
		download(1, 'error'),
		download(2, 'error'),
		download(3, 'downloading'),
		download(4, 'finished')
	];

	const updated = applyRetrySuccess(rows, [1, 2, 3, 4]);

	assert.equal(updated[0].status, 'queued');
	assert.equal(updated[0].error, null);
	assert.equal(updated[1].status, 'queued');
	// A row an event already advanced keeps its newer state instead of regressing to queued
	assert.equal(updated[2].status, 'downloading');
	assert.equal(updated[3].status, 'finished');
});

test('activity snapshots restore only active downloads for the current manga', () => {
	const activity = downloadActivitySnapshot([
		{ ...download(7, 'downloading'), downloaded: 3, total: 8 },
		download(8, 'queued'),
		download(9, 'finished'),
		{ ...download(10, 'downloading'), mangaId: 4 }
	], 3);

	assert.deepEqual(activity, new Map([
		[7, { status: 'downloading', progress: { downloaded: 3, total: 8 } }],
		[8, { status: 'queued', progress: null }]
	]));
});
