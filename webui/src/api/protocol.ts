import type {
	ApiError,
	Download,
	GlobalSearchResult,
	HomeComponent,
	HomeLayout,
	SearchManga,
	SourceManga,
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

export type DownloadWsFrame =
	| { type: 'snapshot'; running: boolean; downloads: Download[] }
	| { type: DownloadEventKind; chapterId: number; downloaded: number; total: number }
	| { type: 'running'; chapterId: 0; downloaded: 0; total: 0; running: boolean };

export type UpdateJobStatus = 'pending' | 'running' | 'complete' | 'failed' | 'skipped';

export interface UpdateJob {
	status: UpdateJobStatus;
	mangaId: number;
	mangaTitle: string;
	sourceId: number;
	addedChapters: number;
}

export type UpdateWsFrame =
	| ({ type: 'summary' } & UpdateSummary)
	| ({ type: 'job' } & UpdateJob);
