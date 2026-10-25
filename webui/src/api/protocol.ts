import type {
	ApiError,
	Download,
	GlobalSearchResult,
	HomeComponent,
	HomeLayout,
	SearchManga,
	SourceManga,
	Task,
	UpdateSummary
} from './types';

export type StreamTerminal<TFinal> =
	| { type: 'final'; payload: TFinal }
	| { type: 'error'; error: ApiError };

export type HomeStreamEvent =
	| { type: 'layout'; layout: HomeLayout }
	| { type: 'component'; index: number; component: HomeComponent };

export type SearchStreamEvent = { type: 'manga'; manga: SearchManga };

export type GlobalSearchStreamEvent =
	| { type: 'source'; source: GlobalSearchResult }
	| { type: 'sourceError'; sourceId: number; error: ApiError };

export type MangaUpdateStreamEvent = { type: 'manga'; manga: SourceManga };

export type DownloadEventKind = 'queued' | 'started' | 'progress' | 'paused' | 'finished' | 'error';

export type DownloadChapterEvent = {
	type: DownloadEventKind;
	chapterId: number;
	downloaded: number;
	total: number;
};

export type EventSnapshot = {
	type: 'snapshot';
	running: boolean;
	downloads: Download[];
	updateSummary: UpdateSummary;
};

export type UpdateJobStatus = 'pending' | 'running' | 'complete' | 'failed' | 'skipped';

export interface UpdateJob {
	status: UpdateJobStatus;
	mangaId: number;
	mangaTitle: string;
	sourceId: number;
	addedChapters: number;
}

export type UpdateEvent = { type: 'job' } & UpdateJob;

/** Named `task` event on /events: the data payload is the Task DTO itself (kinds include local_scan, local_analyze, empty_trash) */
export type TaskEvent = Task;
