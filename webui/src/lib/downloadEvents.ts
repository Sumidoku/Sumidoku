import type { DownloadChapterEvent } from '../api/protocol';
import type { ChapterWithDownload, Download, DownloadStatus } from '../api/types';

// A progress frame can also arrive before started; any progress means downloading.
const EVENT_STATUS: Record<DownloadChapterEvent['type'], DownloadStatus> = {
	queued: 'queued',
	started: 'downloading',
	progress: 'downloading',
	paused: 'paused',
	finished: 'finished',
	error: 'error'
};

export function applyDownloadEvent(
	items: readonly Download[],
	event: DownloadChapterEvent
): Download[] {
	const status = EVENT_STATUS[event.type];
	return items.map((item) => {
		if (item.chapterId !== event.chapterId) return item;
		const patch: Partial<Download> = { status };
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
	current: readonly Download[],
	response: readonly Download[],
	events: ReadonlyMap<number, DownloadChapterEvent>
): Download[] {
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
			error: currentItem.error
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
			updater: (
				chapters: ChapterWithDownload[] | undefined
			) => ChapterWithDownload[] | undefined
		) => void;
		invalidate: (queryKey: readonly unknown[]) => void;
		clearQueued: (chapterId: number) => void;
	}
) {
	return {
		onDownload(event: DownloadChapterEvent) {
			if (event.type !== 'finished' && event.type !== 'error') return;
			actions.clearQueued(event.chapterId);
			if (event.type !== 'finished') return;
			actions.setChapters((chapters) => markChapterDownloaded(chapters, event.chapterId));
			actions.invalidate(chaptersKey);
			actions.invalidate(libraryKey);
		},
		onSnapshot() {
			actions.invalidate(chaptersKey);
		}
	};
}
