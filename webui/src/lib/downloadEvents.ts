import type { DownloadChapterEvent } from './api/protocol';
import type { ChapterWithDownload, DownloadItem, DownloadStatus } from './api/types';

export type DownloadProgress = {
	downloaded: number;
	total: number;
};

export type MangaDownloadActivity = {
	status: Extract<DownloadStatus, 'queued' | 'downloading' | 'paused'>;
	progress: DownloadProgress | null;
};

// progress 帧也可能先于 started 到达，有进度即视为下载中
const EVENT_STATUS: Record<DownloadChapterEvent['type'], DownloadStatus> = {
	queued: 'queued',
	started: 'downloading',
	progress: 'downloading',
	paused: 'paused',
	finished: 'finished',
	error: 'error'
};

export function formatDownloadProgress({ downloaded, total }: DownloadProgress): string {
	const counts = total > 0 ? `${downloaded} / ${total}` : `${downloaded}`;
	if (total <= 0) return counts;
	const percent = Math.min(100, Math.max(0, Math.floor((downloaded / total) * 100)));
	return `${counts} · ${percent}%`;
}

export function downloadActivitySnapshot(
	downloads: readonly DownloadItem[],
	mangaId: number
): Map<number, MangaDownloadActivity> {
	const activity = new Map<number, MangaDownloadActivity>();
	for (const download of downloads) {
		if (download.mangaId !== mangaId) continue;
		const status = download.status;
		if (status !== 'queued' && status !== 'downloading' && status !== 'paused') continue;
		const progress =
			status === 'downloading' && download.downloaded !== null && download.total !== null
				? { downloaded: download.downloaded, total: download.total }
				: null;
		activity.set(download.chapterId, { status, progress });
	}
	return activity;
}

export function applyDownloadEvent(
	items: readonly DownloadItem[],
	event: DownloadChapterEvent
): DownloadItem[] {
	const status = EVENT_STATUS[event.type];
	return items.map((item) => {
		if (item.chapterId !== event.chapterId) return item;
		const patch: Partial<DownloadItem> = { status };
		if (status === 'downloading' || status === 'finished') {
			patch.downloaded = event.downloaded;
			patch.total = event.total;
			if (status === 'finished') {
				patch.error = null;
				if (event.total > 0) patch.downloaded = event.total;
			}
		} else {
			patch.downloaded = null;
			patch.total = null;
			if (status === 'queued') patch.error = null;
		}
		return { ...item, ...patch };
	});
}

export function mergeDownloadResponse(
	current: readonly DownloadItem[],
	response: readonly DownloadItem[],
	events: ReadonlyMap<number, DownloadChapterEvent>
): DownloadItem[] {
	if (events.size === 0) return [...response];
	const currentById = new Map(current.map((item) => [item.chapterId, item]));
	return response.map((item) => {
		const event = events.get(item.chapterId);
		if (!event) return item;
		const currentItem = currentById.get(item.chapterId);
		if (!currentItem) return applyDownloadEvent([item], event)[0];
		return {
			...item,
			status: currentItem.status,
			downloaded: currentItem.downloaded,
			total: currentItem.total,
			error: currentItem.status === 'error' ? item.error : currentItem.error
		};
	});
}

export function markChapterDownloaded(
	chapters: readonly ChapterWithDownload[] | undefined,
	chapterId: number
): ChapterWithDownload[] | undefined {
	return chapters?.map((chapter) =>
		chapter.id === chapterId ? { ...chapter, isDownloaded: true } : chapter
	);
}

export function createMangaDownloadEventHandlers(
	chaptersKey: readonly unknown[],
	libraryKey: readonly unknown[],
	actions: {
		setChapters: (
			updater: (chapters: ChapterWithDownload[] | undefined) => ChapterWithDownload[] | undefined
		) => void;
		invalidate: (queryKey: readonly unknown[]) => void;
		setActivity: (chapterId: number, activity: MangaDownloadActivity | null) => void;
		syncActivity: (downloads: readonly DownloadItem[]) => void;
	}
) {
	return {
		onDownload(event: DownloadChapterEvent) {
			const status = EVENT_STATUS[event.type];
			if (status === 'downloading') {
				actions.setActivity(event.chapterId, {
					status,
					progress: { downloaded: event.downloaded, total: event.total }
				});
				return;
			}
			if (status === 'queued' || status === 'paused') {
				actions.setActivity(event.chapterId, { status, progress: null });
				return;
			}
			actions.setActivity(event.chapterId, null);
			if (event.type !== 'finished') return;
			actions.setChapters((chapters) => markChapterDownloaded(chapters, event.chapterId));
			actions.invalidate(chaptersKey);
			actions.invalidate(libraryKey);
		},
		onSnapshot(downloads: readonly DownloadItem[]) {
			actions.syncActivity(downloads);
			actions.invalidate(chaptersKey);
		}
	};
}
