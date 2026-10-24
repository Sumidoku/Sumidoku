import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { selectDownloadChapterIds } from '../src/lib/downloadRules.ts';
import type { ChapterWithDownload } from '../src/api/types.ts';

function chapter(id: number, overrides: Partial<ChapterWithDownload> = {}): ChapterWithDownload {
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
		isDownloaded: false,
		...overrides
	};
}

test('unread includes started chapters and skips downloaded or placeholder rows', () => {
	const ids = selectDownloadChapterIds([
		chapter(8),
		chapter(7, { isRead: true }),
		chapter(6, { isDownloaded: true }),
		chapter(5, { lastPageRead: 2 }),
		chapter(-1),
		chapter(8)
	], 'unread');

	assert.deepEqual(ids, [8, 5]);
});

test('latest N follows source order and skips downloaded chapters before taking N', () => {
	const ids = selectDownloadChapterIds([
		chapter(9, { chapter_number: 1, isDownloaded: true }),
		chapter(3, { chapter_number: 99 }),
		chapter(8, { chapter_number: 2 }),
		chapter(2, { chapter_number: 100 })
	], 'latest', 2);

	assert.deepEqual(ids, [3, 8]);
});

test('all includes unread and read chapters but not downloaded or duplicate ids', () => {
	const ids = selectDownloadChapterIds([
		chapter(4, { isRead: true }),
		chapter(3, { isDownloaded: true }),
		chapter(2),
		chapter(4)
	], 'all');

	assert.deepEqual(ids, [4, 2]);
});

test('latest N rejects non-positive or fractional counts', () => {
	for (const count of [0, -1, 1.5, Number.NaN]) {
		assert.deepEqual(selectDownloadChapterIds([chapter(1)], 'latest', count), []);
	}
});

test('rule dialog keeps the actual queued count visible', () => {
	const source = readFileSync(new URL('../src/pages/MangaDetailPage.tsx', import.meta.url), 'utf8');
	assert.match(source, /onSuccess: \(res\) => setDownloadRuleQueued\(res\.queued\.length\)/);
	assert.match(source, /<p role="status">\{t\('mangaDetail\.queuedChapters', \{ count: downloadRuleQueued \}\)\}<\/p>/);
});
