import type { ChapterWithDownload } from '../api/types';

export type DownloadRule = 'unread' | 'latest' | 'all';

export function selectDownloadChapterIds(
	chapters: readonly ChapterWithDownload[],
	rule: DownloadRule,
	latestCount = 10
): number[] {
	let candidates = chapters.filter((chapter) => chapter.id > 0 && !chapter.isDownloaded);
	if (rule === 'unread') candidates = candidates.filter((chapter) => !chapter.isRead);
	if (rule === 'latest') {
		if (!Number.isInteger(latestCount) || latestCount < 1) return [];
		candidates = candidates.slice(0, latestCount);
	}
	return [...new Set(candidates.map((chapter) => chapter.id))];
}
