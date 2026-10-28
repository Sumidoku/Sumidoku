import { ApiError, api } from './client';
import { qk } from './hooks';
import type { QueryClient } from '@tanstack/react-query';
import type {
	RemoteState,
	TrackSearchItem,
	TrackStatus,
	TrackUpdate,
	TrackerLedgerItem,
	TrackerLink,
	TrackerLinkPage,
	TrackerLinkUpsert,
	TrackerOAuthStatus,
	TrackerProvider,
	TrackerSyncResult,
	TrackerUpdateRequest
} from './types';

export const TRACKER_PROVIDERS = ['anilist', 'mal', 'bangumi', 'mangabaka'] as const satisfies readonly TrackerProvider[];

export const TRACKER_PROVIDER_NAMES: Record<TrackerProvider, string> = {
	anilist: 'AniList',
	mal: 'MyAnimeList',
	bangumi: 'Bangumi',
	mangabaka: 'MangaBaka'
};

export const trackerKeys = {
	links: ['trackers', 'links'] as const,
	mangaLinks: (mangaId: number) => ['trackers', 'links', mangaId] as const,
	ledger: ['trackers', 'ledger'] as const,
	oauth: ['trackers', 'oauth'] as const,
	oauthStatus: (provider: TrackerProvider) => ['trackers', 'oauth', provider] as const,
	search: (provider: TrackerProvider, name: string, nsfw?: boolean) =>
		['trackers', 'search', provider, name, nsfw] as const,
	states: ['trackers', 'state'] as const,
	state: (provider: TrackerProvider, trackId: string) => ['trackers', 'state', provider, trackId] as const
};

export function invalidateTrackerLinks(qc: QueryClient, mangaId: number) {
	void qc.invalidateQueries({ queryKey: trackerKeys.links });
	void qc.invalidateQueries({ queryKey: qk.library });
	void qc.invalidateQueries({ queryKey: qk.manga(mangaId) });
}

export function listAllTrackerLinks(page: number) {
	return api<TrackerLinkPage>(`/trackers/links?page=${page}`);
}

export function listTrackerLinks(mangaId: number) {
	return api<TrackerLink[]>(`/trackers/links/${mangaId}`);
}

export function putTrackerLink(mangaId: number, body: TrackerLinkUpsert) {
	return api<TrackerLink>(`/trackers/links/${mangaId}`, { method: 'PUT', body });
}

export function deleteTrackerLink(mangaId: number, provider: TrackerProvider) {
	return api<void>(`/trackers/links/${mangaId}/${provider}`, { method: 'DELETE' });
}

/// Aidoku's "Sync Local History": pull the tracker's progress onto the local
/// history. `preview` only computes the plan; applying marks the chapters
/// read and pushes the new progress to the manga's other bindings.
export function syncTrackerLink(mangaId: number, provider: TrackerProvider, preview = false) {
	return api<TrackerSyncResult>(`/trackers/links/${mangaId}/${provider}/sync`, {
		method: 'POST',
		body: { preview },
		silent: true
	});
}

export function trackerLedger() {
	return api<TrackerLedgerItem[]>('/trackers/ledger');
}

export function searchTracker(provider: TrackerProvider, name: string, nsfw?: boolean) {
	const params = new URLSearchParams({ name });
	if (nsfw !== undefined) params.set('nsfw', String(nsfw));
	return api<TrackSearchItem[]>(`/trackers/${provider}/search?${params}`, { silent: true });
}

export function trackerState(provider: TrackerProvider, trackId: string) {
	const params = new URLSearchParams({ trackId });
	return api<RemoteState>(`/trackers/${provider}/state?${params}`, { silent: true });
}

export function updateTracker(provider: TrackerProvider, body: TrackerUpdateRequest) {
	return api<void>(`/trackers/${provider}/update`, { method: 'POST', body, silent: true });
}

export function oauthStatus(provider: TrackerProvider) {
	return api<TrackerOAuthStatus>(`/trackers/oauth/${provider}/status`, { silent: true });
}

export function logoutTracker(provider: TrackerProvider) {
	return api<void>(`/trackers/oauth/${provider}/logout`, { method: 'POST' });
}

export function oauthStartUrl(provider: TrackerProvider) {
	return `/api/v1/trackers/oauth/${provider}/start`;
}

export function trackerEntryUrl(provider: TrackerProvider, trackId: string) {
	const id = encodeURIComponent(trackId);
	switch (provider) {
		case 'anilist':
			return `https://anilist.co/manga/${id}`;
		case 'mal':
			return `https://myanimelist.net/manga/${id}`;
		case 'bangumi':
			return `https://bgm.tv/subject/${id}`;
		case 'mangabaka':
			return `https://mangabaka.org/${id}`;
	}
}

export function isTrackerAuthError(error: unknown): error is ApiError {
	return error instanceof ApiError && error.status === 409 && error.code === 'tracker_auth';
}

export function availableTrackerProviders(links: readonly TrackerLink[]) {
	const bound = new Set(links.map((link) => link.provider));
	return TRACKER_PROVIDERS.filter((provider) => !bound.has(provider));
}

export interface TrackEditValues {
	status: TrackStatus | '';
	score: string;
	lastReadChapter: string;
	lastReadVolume: string;
}

export function remoteToTrackEdit(state: RemoteState): TrackEditValues {
	return {
		status: state.status ?? '',
		score: state.score == null ? '' : String(state.score),
		lastReadChapter: state.lastReadChapter == null ? '' : String(state.lastReadChapter),
		lastReadVolume: state.lastReadVolume == null ? '' : String(state.lastReadVolume)
	};
}

export function mergeTrackEditValues(initial: RemoteState | null, edits: Partial<TrackEditValues>): TrackEditValues {
	return { ...remoteToTrackEdit(initial ?? {}), ...edits };
}

export function updateTrackEditValue<K extends keyof TrackEditValues>(
	initial: RemoteState | null,
	edits: Partial<TrackEditValues>,
	field: K,
	value: TrackEditValues[K]
): Partial<TrackEditValues> {
	const next = { ...edits };
	if (remoteToTrackEdit(initial ?? {})[field] === value) delete next[field];
	else next[field] = value;
	return next;
}

function editedNumber(value: string) {
	if (!value.trim()) return 0;
	const parsed = Number(value);
	return Number.isFinite(parsed) ? parsed : undefined;
}

export function buildTrackUpdate(initial: RemoteState, values: TrackEditValues): TrackUpdate {
	const initialValues = remoteToTrackEdit(initial);
	const update: TrackUpdate = {};
	if (values.status !== initialValues.status && values.status) update.status = values.status;

	const score = editedNumber(values.score);
	if (values.score !== initialValues.score && score !== undefined && score !== initial.score) {
		update.score = score;
	}
	const chapter = editedNumber(values.lastReadChapter);
	if (
		values.lastReadChapter !== initialValues.lastReadChapter &&
		chapter !== undefined &&
		chapter !== initial.lastReadChapter
	) {
		update.lastReadChapter = chapter;
	}
	const volume = editedNumber(values.lastReadVolume);
	if (
		values.lastReadVolume !== initialValues.lastReadVolume &&
		volume !== undefined &&
		volume !== initial.lastReadVolume
	) {
		update.lastReadVolume = volume;
	}
	return update;
}

export function hasTrackUpdate(update: TrackUpdate) {
	return Object.keys(update).length > 0;
}

export function buildTrackerSave(
	link: TrackerLink,
	trackMode: TrackerLink['trackMode'],
	chapterOffset: number,
	initialState: RemoteState | null,
	values: TrackEditValues
) {
	const linkUpdate =
		trackMode === link.trackMode && chapterOffset === link.chapterOffset
			? undefined
			: {
					provider: link.provider,
					trackId: link.trackId,
					title: link.title,
					trackMode,
					chapterOffset
				};
	return {
		link: linkUpdate,
		remoteUpdate: initialState ? buildTrackUpdate(initialState, values) : {}
	};
}

export function readOAuthReturn(search: string) {
	const params = new URLSearchParams(search);
	const status = params.get('oauth');
	if (status !== 'success' && status !== 'error') return null;
	const message = params.get('message');
	params.delete('oauth');
	params.delete('message');
	return { status, message, cleanedSearch: params.toString() };
}
