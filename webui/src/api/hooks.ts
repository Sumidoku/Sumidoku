import {
	QueryClient,
	useInfiniteQuery,
	useMutation,
	useQuery,
	useQueryClient
} from '@tanstack/react-query';
import type { InfiniteData } from '@tanstack/react-query';
import { ApiError, api, apiStream } from './client';
import type {
	AidokuImportResponse,
	ApiKeyCreated,
	ApiKeyEntry,
	AuthStatus,
	BackupFiles,
	BackupRestoreResponse,
	Category,
	ChapterCacheClearResponse,
	ChapterCacheStats,
	ChapterProgress,
	ChapterWithDownload,
	DeepLinkResponse,
	FilterGroup,
	FilterRule,
	FilterValue,
	GlobalSearchResponse,
	GlobalSearchResult,
	HistoryPage,
	HomeLayout,
	LibraryPage,
	LibrarySort,
	Listing,
	LocalTrashEntry,
	Manga,
	MangaInput,
	MangaPage,
	MigrationLookupResponse,
	MigrateRequest,
	MigrateResponse,
	MigrationSource,
	NameCount,
	Page,
	ProgressPatch,
	QueuedResponse,
	RecentPage,
	RegisterInternalRequest,
	RepoSource,
	ScanOutcome,
	ServerSettings,
	Source,
	SourceCount,
	SourceFilter,
	SourceRepo,
	SourceSettings,
	SourceWithMigration,
	SuwayomiImportResponse,
	Task,
	UpdateAllSourcesResponse,
	UpdateMangaRequest
} from './types';
import type {
	GlobalSearchStreamEvent,
	HomeStreamEvent,
	MangaUpdateStreamEvent,
	SearchStreamEvent
} from './protocol';
import { sse } from './sse';
import type { TriStateSelection } from '../lib/triState';

export const queryClient = new QueryClient({
	defaultOptions: {
		// gcTime bounds how long an unobserved cache entry lingers; the default 5 minutes meant that
		// returning from a long reader/detail session remounted the previous list with a full refetch
		// (and lost its scroll position). A day covers any realistic session. The heaviest payload —
		// chapter lists with a per-row thumbnail string — stays at this bound rather than Infinity
		queries: { staleTime: 30_000, gcTime: 24 * 60 * 60_000, retry: 1 }
	}
});

// Single source of query keys; cross-page invalidation (e.g. refreshing the library after adding a manga) reconciles by prefix
export const qk = {
	sources: ['sources'] as const,
	sourceRepos: ['source-repos'] as const,
	repoSources: ['source-repos', 'sources'] as const,
	categories: ['categories'] as const,
	filterGroups: ['filter-groups'] as const,
	library: ['library'] as const,
	manga: (id: number) => ['manga', id] as const,
	chapters: (mangaId: number) => ['manga', mangaId, 'chapters'] as const,
	sourceFilters: (sourceId: number) => ['sources', sourceId, 'filters'] as const,
	sourceListings: (sourceId: number) => ['sources', sourceId, 'listings'] as const,
	sourceHome: (sourceId: number) => ['sources', sourceId, 'home'] as const,
	backups: ['backups'] as const,
	whoami: ['whoami'] as const,
	apiKeys: ['api-keys'] as const,
	chapterCache: ['chapter-cache'] as const,
	tasks: ['tasks'] as const,
	localTrash: ['local-trash'] as const
};

const nextPage = (last: { hasNextPage: boolean }, all: { hasNextPage: boolean }[]) =>
	last.hasNextPage ? all.length + 1 : undefined;

/** When dynamic capabilities (filters/listings) are unsupported the server returns 501 with code unsupported; treat as an empty list, not an error */
async function emptyOnUnsupported<T>(p: Promise<T[]>): Promise<T[]> {
	try {
		return await p;
	} catch (e) {
		if (e instanceof ApiError && e.code === 'unsupported') return [];
		throw e;
	}
}

/** When the home capability is unsupported, treat as null so pages show "this source has no home" instead of an error */
async function nullOnUnsupported<T>(p: Promise<T>): Promise<T | null> {
	try {
		return await p;
	} catch (e) {
		if (e instanceof ApiError && e.code === 'unsupported') return null;
		throw e;
	}
}

// ---- Queries ----

export function useSources() {
	return useQuery({ queryKey: qk.sources, queryFn: () => api<Source[]>('/sources') });
}

// ---- Auth identity ----

/** The usual identity source outside the login/settings pages; silent: a logged-out 401 goes to the global redirect without a toast */
export function useWhoami() {
	return useQuery({
		queryKey: qk.whoami,
		queryFn: () => api<AuthStatus>('/auth/status', { silent: true })
	});
}

// ---- API keys ----

export function useApiKeys() {
	return useQuery({ queryKey: qk.apiKeys, queryFn: () => api<ApiKeyEntry[]>('/auth/api-keys') });
}

/** silent: the caller turns failures into a toast so the created key can be shown on success */
export function useCreateApiKey() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (name: string) =>
			api<ApiKeyCreated>('/auth/api-keys', { method: 'POST', body: { name }, silent: true }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.apiKeys })
	});
}

export function useRevokeApiKey() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (id: string) => api(`/auth/api-keys/${id}`, { method: 'DELETE', silent: true }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.apiKeys })
	});
}

/** Pick a single source out of the source list; there is no standalone GET /sources/{id} endpoint */
export function useSource(id: number) {
	return useQuery({
		queryKey: qk.sources,
		queryFn: () => api<Source[]>('/sources'),
		select: (list) => list.find((s) => s.id === id)
	});
}

export interface LibraryBrowse {
	search?: string;
	sort?: LibrarySort;
	/** Boolean filter tri-state (unread/downloaded/bookmarked keys): 1 include / -1 exclude */
	filters?: TriStateSelection;
	/** The migration batch flow filters entries by source */
	sourceId?: number;
	/** Content rating tri-state (manga-level 1..=3 → 1 include / -1 exclude) */
	ratings?: TriStateSelection;
	/** Source tri-state (source id → 1 include / -1 exclude) */
	sources?: TriStateSelection;
	/** Category tri-state (category id, "0" = uncategorized → 1 include / -1 exclude) */
	categories?: TriStateSelection;
	/** Author tri-state (author name → 1 include / -1 exclude); exact, case-insensitive server-side */
	authors?: TriStateSelection;
	/** Tag tri-state (tag name → 1 include / -1 exclude); exact, case-insensitive server-side */
	tags?: TriStateSelection;
	/** The active filter group; its rules compile server-side and AND with the ad-hoc filters */
	groupId?: number;
}

/** Split a tri-state into the API's two comma-separated include/exclude strings; an empty set is omitted */
function triParams(sel: TriStateSelection | undefined): [string?, string?] {
	if (!sel) return [undefined, undefined];
	const inc = Object.entries(sel)
		.filter(([, v]) => v === 1)
		.map(([k]) => k);
	const exc = Object.entries(sel)
		.filter(([, v]) => v === -1)
		.map(([k]) => k);
	return [inc.length > 0 ? inc.join(',') : undefined, exc.length > 0 ? exc.join(',') : undefined];
}

export function useLibrary(browse: LibraryBrowse = {}) {
	const { search, sort, filters, sourceId, ratings, sources, categories, authors, tags, groupId } = browse;
	const [filterInc, filterExc] = triParams(filters);
	const [ratingInc, ratingExc] = triParams(ratings);
	const [sourceInc, sourceExc] = triParams(sources);
	const [categoryInc, categoryExc] = triParams(categories);
	const [authorInc, authorExc] = triParams(authors);
	const [tagInc, tagExc] = triParams(tags);
	return useInfiniteQuery({
		queryKey: [...qk.library, { search, sort, filters, sourceId, ratings, sources, categories, authors, tags, groupId }],
		queryFn: ({ pageParam }) => {
			const qs = new URLSearchParams({ page: String(pageParam) });
			if (sourceId !== undefined) qs.set('sourceId', String(sourceId));
			if (search) qs.set('search', search);
			if (sort) qs.set('sort', sort);
			if (filterInc) qs.set('filter', filterInc);
			if (filterExc) qs.set('filterExclude', filterExc);
			if (ratingInc) qs.set('contentRating', ratingInc);
			if (ratingExc) qs.set('contentRatingExclude', ratingExc);
			if (sourceInc) qs.set('source', sourceInc);
			if (sourceExc) qs.set('sourceExclude', sourceExc);
			if (categoryInc) qs.set('category', categoryInc);
			if (categoryExc) qs.set('categoryExclude', categoryExc);
			if (authorInc) qs.set('author', authorInc);
			if (authorExc) qs.set('authorExclude', authorExc);
			if (tagInc) qs.set('tag', tagInc);
			if (tagExc) qs.set('tagExclude', tagExc);
			if (groupId !== undefined) qs.set('groupId', String(groupId));
			return api<LibraryPage>(`/library?${qs}`);
		},
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

export function useCategories() {
	return useQuery({ queryKey: qk.categories, queryFn: () => api<Category[]>('/categories') });
}

/** Per-source entry counts in the library; the queryKey hangs under qk.library so add/remove invalidates and refetches it automatically */
export function useLibrarySourceCounts(enabled = true) {
	return useQuery({
		queryKey: [...qk.library, 'source-counts'],
		queryFn: () => api<SourceCount[]>('/library/source-counts'),
		enabled
	});
}

/** Distinct authors across the user's library with entry counts; hangs under qk.library so library invalidations refetch it */
export function useLibraryAuthors(enabled = true) {
	return useQuery({
		queryKey: [...qk.library, 'authors'],
		queryFn: () => api<NameCount[]>('/library/authors'),
		enabled
	});
}

/** Distinct tags across the user's library with entry counts; hangs under qk.library so library invalidations refetch it */
export function useLibraryTags(enabled = true) {
	return useQuery({
		queryKey: [...qk.library, 'tags'],
		queryFn: () => api<NameCount[]>('/library/tags'),
		enabled
	});
}

export function useSourceFilters(sourceId: number, enabled = true) {
	return useQuery({
		queryKey: qk.sourceFilters(sourceId),
		queryFn: () => emptyOnUnsupported(api<SourceFilter[]>(`/sources/${sourceId}/filters`, { silent: true })),
		enabled
	});
}

export function useSourceListings(sourceId: number, enabled = true) {
	return useQuery({
		queryKey: qk.sourceListings(sourceId),
		queryFn: () => emptyOnUnsupported(api<Listing[]>(`/sources/${sourceId}/listings`, { silent: true })),
		enabled
	});
}

export function useSourceHome(sourceId: number, enabled = true) {
	const qc = useQueryClient();
	return useQuery({
		queryKey: qk.sourceHome(sourceId),
		queryFn: ({ signal }) =>
			nullOnUnsupported(
				apiStream<HomeLayout, HomeStreamEvent>(`/sources/${sourceId}/home`, {
					silent: true,
					signal,
					onEvent: (ev) => {
						// partials are written to the cache as they arrive so the page renders edge-to-edge; the terminal state overwrites once queryFn returns
						qc.setQueryData<HomeLayout | null>(qk.sourceHome(sourceId), (cur) => {
							if (ev.type === 'layout') return ev.layout;
							if (ev.type === 'component') {
								const components = (cur?.components ?? []).slice();
								components[ev.index] = ev.component;
								return { components };
							}
							return cur ?? null;
						});
					}
				})
			),
		enabled
	});
}

export interface SourceSearchRequest {
	query?: string;
	filters: FilterValue[];
}

export function useSourceSearch(sourceId: number, req: SourceSearchRequest, enabled = true) {
	const qc = useQueryClient();
	const queryKey = ['sources', sourceId, 'search', req] as const;
	return useInfiniteQuery({
		queryKey,
		queryFn: ({ pageParam, signal }) =>
			apiStream<MangaPage, SearchStreamEvent>(`/sources/${sourceId}/search`, {
				method: 'POST',
				body: { query: req.query || undefined, page: pageParam, filters: req.filters },
				signal,
				onEvent: (ev) => {
					if (ev.type !== 'manga') return;
					// partials append one by one to their page; the terminal state overwrites the whole page once queryFn returns
					qc.setQueryData<InfiniteData<MangaPage, number>>(queryKey, (cur) => {
						const idx = pageParam - 1;
						if (!cur) {
							if (idx !== 0) return cur;
							return { pages: [{ entries: [ev.manga], hasNextPage: true }], pageParams: [1] };
						}
						const page = cur.pages[idx];
						if (!page) return cur;
						const pages = cur.pages.slice();
						pages[idx] = { ...page, entries: [...page.entries, ev.manga] };
						return { ...cur, pages };
					});
				}
			}),
		initialPageParam: 1,
		getNextPageParam: nextPage,
		enabled
	});
}

/** Cross-source aggregate search: each source's streamed result is written into the aggregate cache by sourceId (rendered as soon as one completes),
 * and the terminal state overwrites everything once returned; the page renders each source's result in its own source order */
export function useGlobalSearch(sourceIds: number[], query: string, enabled = true) {
	const qc = useQueryClient();
	const queryKey = ['global-search', query, sourceIds] as const;
	return useQuery({
		queryKey,
		queryFn: ({ signal }) =>
			apiStream<GlobalSearchResponse, GlobalSearchStreamEvent>('/search', {
				method: 'POST',
				body: { query: query || undefined, page: 1, sourceIds },
				signal,
				onEvent: (ev) => {
					const result: GlobalSearchResult =
						ev.type === 'source'
							? ev.source
							: { sourceId: ev.sourceId, entries: [], hasNextPage: false, error: ev.error };
					qc.setQueryData<GlobalSearchResponse>(queryKey, (cur) => {
						const rest = (cur?.sources ?? []).filter((s) => s.sourceId !== result.sourceId);
						return { sources: [...rest, result] };
					});
				}
			}),
		enabled
	});
}

export function useListing(sourceId: number, listingId: string, enabled = true) {
	return useInfiniteQuery({
		queryKey: ['sources', sourceId, 'listing', listingId],
		queryFn: ({ pageParam }) =>
			api<MangaPage>(
				`/sources/${sourceId}/listings/${encodeURIComponent(listingId)}?page=${pageParam}`
			),
		initialPageParam: 1,
		getNextPageParam: nextPage,
		enabled
	});
}

// ---- Mutations ----

export function useRegisterManga() {
	return useMutation({
		mutationFn: (body: { sourceId: number; manga: MangaInput }) =>
			apiStream<{ id: number }, MangaUpdateStreamEvent>('/manga', {
				method: 'POST',
				body,
				// the internal id only exists once registration completes; partials have no page state to attach to, so ignore them
				onEvent: () => {}
			})
	});
}

export function useDeepLink() {
	return useMutation({
		mutationFn: ({ sourceId, url }: { sourceId: number; url: string }) =>
			api<DeepLinkResponse>(`/sources/${sourceId}/deep-link`, { method: 'POST', body: { url } })
	});
}

export function useAddToLibrary() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) => api('/library', { method: 'POST', body: { mangaId } }),
		onSuccess: (_d, mangaId) => {
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: qk.manga(mangaId) });
		}
	});
}

export function useRemoveFromLibrary() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) => api(`/library/${mangaId}`, { method: 'DELETE' }),
		onSuccess: (_d, mangaId) => {
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: qk.manga(mangaId) });
		}
	});
}

export function useSetMangaCategories() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: { mangaId: number; categoryIds: number[] }) =>
			api(`/manga/${body.mangaId}/categories`, {
				method: 'PUT',
				body: { categoryIds: body.categoryIds }
			}),
		onSuccess: (_d, body) => {
			void qc.invalidateQueries({ queryKey: qk.manga(body.mangaId) });
			void qc.invalidateQueries({ queryKey: qk.categories });
			void qc.invalidateQueries({ queryKey: qk.library });
		}
	});
}

export function usePatchManga() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, ...body }: { id: number } & UpdateMangaRequest) =>
			api<Manga>(`/manga/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d, { id }) => void qc.invalidateQueries({ queryKey: qk.manga(id) })
	});
}

// the chapters cache is keyed by mangaId while the PATCH response only carries chapterId, so callers must pass mangaId themselves to invalidate the chapter list
export function useUpdateChapter() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, mangaId: _mangaId, ...body }: { id: number; mangaId?: number } & ProgressPatch) =>
			api<ChapterProgress>(`/chapter/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d, { mangaId }) => {
			if (mangaId !== undefined) void qc.invalidateQueries({ queryKey: qk.chapters(mangaId) });
		}
	});
}

export function useBatchChapters() {
	return useMutation({
		mutationFn: (body: { chapterIds?: number[]; mangaIds?: number[] } & ProgressPatch) =>
			api<{ updated: number }>('/chapter/batch', { method: 'POST', body })
	});
}

export type EnqueueDownloadsInput = number[] | { chapterIds?: number[]; mangaIds?: number[] };

export function useEnqueueDownloads() {
	return useMutation({
		mutationFn: (input: EnqueueDownloadsInput) =>
			// the bare-array form predates manga-level enqueue and maps to chapterIds
			api<QueuedResponse>('/downloads', {
				method: 'POST',
				body: Array.isArray(input) ? { chapterIds: input } : input
			})
	});
}

/** Uses the same row/artifact removal as single-chapter delete, including cancellation of active rows */
export function useDeleteDownloads() {
	return useMutation({
		mutationFn: (mangaIds: number[]) =>
			api<{ deleted: number }>('/downloads/delete', { method: 'POST', body: { mangaIds } })
	});
}

export function useScanSources() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () => api<ScanOutcome>('/sources/scan', { method: 'POST' }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

export function useImportSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (file: File) => {
			const form = new FormData();
			form.append('file', file);
			return api<SourceWithMigration>('/sources/import', { method: 'POST', formData: form });
		},
		onSuccess: () => invalidateSourceViews(qc)
	});
}

export function useRegisterInternalSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (req: RegisterInternalRequest) =>
			api<Source>('/sources/internal', { method: 'POST', body: req }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

export function useDeleteSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (sourceId: number) => api(`/sources/${sourceId}`, { method: 'DELETE' }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

export function usePinSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, pinned }: { id: number; pinned: boolean }) =>
			api<Source>(`/sources/${id}`, { method: 'PATCH', body: { pinned } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

export function useSetSourceEnabled() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
			api<Source>(`/sources/${id}`, { method: 'PATCH', body: { enabled } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

/** ids is an ordered array of source keys; the server requires a one-to-one match with the current pinned set, so don't call it when the order hasn't changed */
export function useReorderPinned() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (ids: string[]) =>
			api<string[]>('/sources/reorder-pinned', { method: 'POST', body: { ids } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

// ---- Local sources: scan tasks and trash ----

export function useTasks() {
	return useQuery({ queryKey: qk.tasks, queryFn: () => api<Task[]>('/tasks') });
}

/** True while a local scan/analyze task is pending or running; kept current by SSE task events (subscribeTaskEvents). A global task (sourceId null) counts for every source */
export function useLocalScanActive(sourceId?: number) {
	const tasks = useTasks();
	return (
		tasks.data?.some(
			(task) =>
				(task.kind === 'local_scan' || task.kind === 'local_analyze') &&
				(task.status === 'pending' || task.status === 'running') &&
				(sourceId === undefined || task.sourceId === null || task.sourceId === sourceId)
		) ?? false
	);
}

export function useLocalTrash() {
	return useQuery({ queryKey: qk.localTrash, queryFn: () => api<LocalTrashEntry[]>('/local/trash') });
}

/** Scanning is background work: the POST only enqueues, progress arrives through SSE task events */
export function useScanLocal() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (sourceId: number | null) =>
			api<Task[]>('/local/scan', { method: 'POST', body: { sourceId } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.tasks })
	});
}

// silent: a 409 'gone' (the trashed directory is still missing on disk) is turned into the conflict toast by the caller
export function useRestoreTrash() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) =>
			api<{ ok: boolean }>(`/local/trash/${mangaId}/restore`, { method: 'POST', silent: true }),
		onSuccess: (_d, mangaId) => {
			void qc.invalidateQueries({ queryKey: qk.localTrash });
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: qk.manga(mangaId) });
		}
	});
}

export function usePurgeTrash() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) => api(`/local/trash/${mangaId}`, { method: 'DELETE' }),
		onSuccess: () => {
			void qc.invalidateQueries({ queryKey: qk.localTrash });
			void qc.invalidateQueries({ queryKey: qk.library });
		}
	});
}

/** Emptying the trash enqueues one empty_trash task per instance; the task events then refresh the list */
export function useEmptyTrash() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (sourceId?: number) =>
			api<Task[]>(`/local/trash${sourceId !== undefined ? `?sourceId=${sourceId}` : ''}`, { method: 'DELETE' }),
		onSuccess: () => {
			void qc.invalidateQueries({ queryKey: qk.tasks });
			void qc.invalidateQueries({ queryKey: qk.localTrash });
		}
	});
}

const TERMINAL_TASK_STATUSES: Task['status'][] = ['done', 'failed', 'cancelled'];

/**
 * Single owner of task-event-driven invalidation (subscribed once from AppLayout): every task event refreshes
 * the task list; a terminal local_scan/local_analyze refreshes everything the scan may have changed (library,
 * manga/chapter details, source browse/search caches and the trash), a terminal empty_trash refreshes the
 * trash and the manga views
 */
export function subscribeTaskEvents(qc: QueryClient): () => void {
	return sse.on('task', (data) => {
		const task = data as Task;
		void qc.invalidateQueries({ queryKey: qk.tasks });
		if (!TERMINAL_TASK_STATUSES.includes(task.status)) return;
		if (task.kind === 'local_scan' || task.kind === 'local_analyze') {
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: ['manga'] });
			void qc.invalidateQueries({ queryKey: ['sources'] });
			void qc.invalidateQueries({ queryKey: ['global-search'] });
			void qc.invalidateQueries({ queryKey: qk.localTrash });
		} else if (task.kind === 'empty_trash') {
			void qc.invalidateQueries({ queryKey: qk.localTrash });
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: ['manga'] });
		}
	});
}

// ---- Source repos ----

export interface RepoBrowseFilter {
	/** Multi-select language codes, comma-joined; an empty array means no filtering */
	langs?: string[];
	query?: string;
	/** Multi-select content ratings, comma-joined; undefined means no filtering, an empty string means none selected */
	contentRating?: string;
}

export function useSourceRepos() {
	return useQuery({
		queryKey: qk.sourceRepos,
		queryFn: () => api<SourceRepo[]>('/source-repos')
	});
}

export function useRepoSources(filter: RepoBrowseFilter, enabled = true) {
	return useQuery({
		queryKey: [...qk.repoSources, filter],
		queryFn: () => {
			const qs = new URLSearchParams();
			if (filter.langs?.length) qs.set('lang', filter.langs.join(','));
			if (filter.query) qs.set('query', filter.query);
			// explicit distinction: undefined means no filtering; an empty string means no rating is selected (the server matches the empty set)
			if (filter.contentRating !== undefined) qs.set('contentRating', filter.contentRating);
			const suffix = qs.size > 0 ? `?${qs}` : '';
			return api<RepoSource[]>(`/source-repos/sources${suffix}`);
		},
		enabled
	});
}

/** After install/update, installedVersion/updateVersion must be recomputed both in the installed list and in every filtered result */
function invalidateSourceViews(qc: QueryClient) {
	void qc.invalidateQueries({ queryKey: qk.sources });
	void qc.invalidateQueries({ queryKey: qk.repoSources });
}

export function useAddSourceRepo() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (url: string) =>
			api<SourceRepo>('/source-repos', { method: 'POST', body: { url } }),
		onSuccess: () => {
			void qc.invalidateQueries({ queryKey: qk.sourceRepos });
			void qc.invalidateQueries({ queryKey: qk.repoSources });
		}
	});
}

export function useDeleteSourceRepo() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (repoId: number) => api(`/source-repos/${repoId}`, { method: 'DELETE' }),
		onSuccess: () => {
			void qc.invalidateQueries({ queryKey: qk.sourceRepos });
			void qc.invalidateQueries({ queryKey: qk.repoSources });
		}
	});
}

export function useRefreshSourceRepo() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (repoId: number) =>
			api<SourceRepo>(`/source-repos/${repoId}/refresh`, { method: 'POST' }),
		onSettled: () => {
			void qc.invalidateQueries({ queryKey: qk.sourceRepos });
			void qc.invalidateQueries({ queryKey: qk.repoSources });
		}
	});
}

export function useInstallRepoSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: { repoId: number; sourceKey: string }) =>
			api<Source>('/source-repos/install', { method: 'POST', body }),
		onSuccess: () => invalidateSourceViews(qc)
	});
}

export function useUpdateSource() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (sourceId: number) =>
			api<SourceWithMigration>(`/sources/${sourceId}/update`, { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(qc)
	});
}

export function useUpdateAllSources() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () => api<UpdateAllSourcesResponse>('/source-repos/update-all', { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(qc)
	});
}

// ---- manga detail/chapter queries ----

export interface MangaQueryOpts {
	enabled?: boolean;
	/** The reader treats detail/chapters as navigation info only; failing to fetch them should not pop a global error toast */
	silent?: boolean;
}

export function useManga(id: number, opts: MangaQueryOpts = {}) {
	return useQuery({
		queryKey: qk.manga(id),
		queryFn: () => api<Manga>(`/manga/${id}`, { silent: opts.silent }),
		enabled: opts.enabled ?? true,
		// The detail page gates its whole render on this query and unmounts while the reader is open;
		// the default 5-minute gc would drop the cache mid-session and returning would flash a full
		// skeleton until the refetch lands. Never evict; updates still arrive via invalidation.
		gcTime: Infinity
	});
}

export function useMangaChapters(mangaId: number, opts: MangaQueryOpts = {}) {
	return useQuery({
		queryKey: qk.chapters(mangaId),
		queryFn: () => api<ChapterWithDownload[]>(`/manga/${mangaId}/chapters`, { silent: opts.silent }),
		enabled: opts.enabled ?? true
		// gcTime: the global 24h bound (see queryClient) — never Infinity here, chapter rows carry a
		// per-row thumbnail string and a pathological source can embed base64 data URIs in it
	});
}

/** Pages are revalidated on entry because cache clearing may replace the backing image revision. */
export function useChapterPages(chapterId: number) {
	return useQuery(chapterPagesQueryOptions(chapterId));
}

export function chapterPagesQueryOptions(chapterId: number) {
	return {
		queryKey: ['chapter', chapterId, 'pages'],
		queryFn: () => api<Page[]>(`/chapter/${chapterId}/pages`),
		staleTime: 0,
		refetchOnMount: 'always' as const
	};
}

// ?refresh pulls from the source with unbounded latency, so it goes through a mutation for the pending state; on success the cache is written directly, saving a refetch
export function useRefreshManga() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (id: number) =>
			apiStream<Manga, MangaUpdateStreamEvent>(`/manga/${id}?refresh`, {
				onEvent: (ev) => {
					if (ev.type !== 'manga') return;
					// detail snapshots render progressively; chapters don't go into the detail cache — the terminal state overwrites everything on return
					const { chapters, ...detail } = ev.manga;
					qc.setQueryData<Manga>(qk.manga(id), (cur) => (cur ? { ...cur, ...detail } : cur));
				}
			}),
		onSuccess: (data, id) => qc.setQueryData(qk.manga(id), data)
	});
}

/** Alternate covers: 501 is the normal branch for a source that doesn't export them; silent lets the page show "no alternates" instead of an error */
export function useAlternateCovers(mangaId: number) {
	return useQuery({
		queryKey: ['manga', mangaId, 'alternate-covers'],
		queryFn: () => api<string[]>(`/manga/${mangaId}/covers/alternates`, { silent: true })
	});
}

// the response is written straight back into the detail cache: once the cover field changes, coverUrl's version param changes with it and the browser refetches the image
export function useSelectCover() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, url }: { id: number; url: string }) =>
			api<Manga>(`/manga/${id}/cover`, { method: 'PUT', body: { url } }),
		onSuccess: (data, { id }) => {
			qc.setQueryData(qk.manga(id), data);
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: ['history'] });
			void qc.invalidateQueries({ queryKey: ['update-recent'] });
		}
	});
}

export function useRefreshChapters() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) =>
			apiStream<ChapterWithDownload[], MangaUpdateStreamEvent>(`/manga/${mangaId}/chapters?refresh`, {
				onEvent: (ev) => {
					if (ev.type !== 'manga' || !ev.manga.chapters) return;
					// chapter snapshots render progressively: source-side chapters have no library id/progress yet, so negative ids are placeholders,
					// and the page uses them to block read/select/download until the terminal state overwrites everything on return
					const partial: ChapterWithDownload[] = ev.manga.chapters.map((c, i) => ({
						...c,
						id: -(i + 1),
						trashed: false,
						pageCount: 0,
						lastPageRead: 0,
						isRead: false,
						isBookmarked: false,
						lastReadAt: null,
						isDownloaded: false,
						hasFinishedDownload: false
					}));
					qc.setQueryData(qk.chapters(mangaId), partial);
				}
			}),
		onSuccess: (data, mangaId) => qc.setQueryData(qk.chapters(mangaId), data)
	});
}

// ---- history / updates pagination ----

export function useHistory() {
	return useInfiniteQuery({
		queryKey: ['history'],
		queryFn: ({ pageParam }) => api<HistoryPage>(`/history?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

export function useUpdateRecent() {
	return useInfiniteQuery({
		queryKey: ['update-recent'],
		queryFn: ({ pageParam }) => api<RecentPage>(`/update/recent?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

// Download list state is driven by /events and owned by the page; only operation hooks live here
export function useDownloadsStart() {
	return useMutation({
		mutationFn: () => api<{ running: boolean }>('/downloads/start', { method: 'POST' })
	});
}

export function useDownloadsStop() {
	return useMutation({
		mutationFn: () => api<{ running: boolean }>('/downloads/stop', { method: 'POST' })
	});
}

export function useDownloadsClear() {
	return useMutation({
		mutationFn: () => api<{ cleared: number }>('/downloads/clear', { method: 'POST' })
	});
}

export function useDeleteDownload() {
	return useMutation({
		mutationFn: (chapterId: number) => api(`/downloads/${chapterId}`, { method: 'DELETE' })
	});
}

export function useDeleteFinishedDownloads() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (chapterIds: number[]) =>
			api<{ deleted: number }>('/downloads/delete', {
				method: 'POST',
				body: { chapterIds, finishedOnly: true }
			}),
		onSettled: () => {
			if (typeof window !== 'undefined') window.dispatchEvent(new Event('sumidoku:downloads-invalidated'));
			void qc.invalidateQueries({ queryKey: ['downloads'] });
			void qc.invalidateQueries({ queryKey: ['manga'] });
			void qc.invalidateQueries({ queryKey: qk.library });
			void qc.invalidateQueries({ queryKey: ['chapter'] });
		}
	});
}

// 409 busy is turned into a hint by the page; silent keeps the client from popping another error
export function useFetchUpdate() {
	return useMutation({
		mutationFn: (categoryId?: number) =>
			api<{ started: boolean }>('/update/fetch', {
				method: 'POST',
				body: categoryId !== undefined ? { categoryId } : {},
				silent: true
			})
	});
}

// ---- settings ----

export function useServerSettings() {
	return useQuery({
		queryKey: ['server-settings'],
		queryFn: () => api<ServerSettings>('/settings')
	});
}

export function useChapterCacheStats() {
	return useQuery({
		queryKey: qk.chapterCache,
		queryFn: () => api<ChapterCacheStats>('/cache/chapters')
	});
}

export function useClearChapterCache() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () => api<ChapterCacheClearResponse>('/cache/chapters/clear', { method: 'POST' }),
		onSuccess: (result) => {
			qc.setQueryData(qk.chapterCache, (current: ChapterCacheStats | undefined) => current
				? { ...current, usedBytes: result.usedBytes, chapterCount: result.chapterCount, cacheRevision: result.cacheRevision }
				: current);
		},
		onSettled: () => {
			void qc.invalidateQueries({ queryKey: ['chapter'] });
			void qc.invalidateQueries({ queryKey: qk.chapterCache });
		}
	});
}

export function useSaveServerSettings() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: ServerSettings) => api<ServerSettings>('/settings', { method: 'PUT', body }),
		onSuccess: (saved) => qc.setQueryData(['server-settings'], saved),
		onSettled: () => {
			void qc.invalidateQueries({ queryKey: qk.chapterCache });
		}
	});
}

type DeepPartial<T> = {
	[P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];
};

/** Merge a partial update into the current structured server settings and return the full object for PUT. */
export function mergeServerSettings(
	current: ServerSettings | undefined,
	patch: DeepPartial<ServerSettings>
): ServerSettings {
	return {
		...current,
		incognito: { ...current?.incognito, ...patch.incognito },
		library: { ...current?.library, ...patch.library },
		download: { ...current?.download, ...patch.download },
		backup: { ...current?.backup, ...patch.backup },
		browse: { ...current?.browse, ...patch.browse },
		cache: { ...current?.cache, ...patch.cache }
	} as ServerSettings;
}

/** Restoring a backup: any section may change, so invalidate everything */
export function useRestoreBackup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ doc, sections }: { doc: unknown; sections: string[] }) =>
			api<BackupRestoreResponse>(`/backups/document?sections=${sections.join(',')}`, {
				method: 'POST',
				body: doc
			}),
		onSuccess: () => qc.invalidateQueries()
	});
}

export function useBackupFiles() {
	return useQuery({
		queryKey: qk.backups,
		queryFn: () => api<BackupFiles>('/backups/files')
	});
}

/** Restore from an existing file in the server backup directory: same import path as upload restore, same full invalidation */
export function useRestoreNamedBackup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (name: string) =>
			api<BackupRestoreResponse>('/backups/files/restore', { method: 'POST', body: { name } }),
		onSuccess: () => qc.invalidateQueries()
	});
}

/** Import an Aidoku .aib: dryRun only previews without writing; a real import invalidates everything */
export function useImportAidoku() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ file, dryRun }: { file: File; dryRun: boolean }) =>
			api<AidokuImportResponse>(`/backups/import/aidoku?dryRun=${dryRun}`, { method: 'POST', body: file }),
		onSuccess: (_result, vars) => {
			if (!vars.dryRun) qc.invalidateQueries();
		}
	});
}

/** Import a Suwayomi/Mihon backup: mapping only applies to unmatched groups, keyed by the Mihon source id in string form */
export function useImportSuwayomi() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({
			file,
			dryRun,
			mapping
		}: {
			file: File;
			dryRun: boolean;
			mapping?: Record<string, string>;
		}) => {
			let query = `dryRun=${dryRun}`;
			if (mapping && Object.keys(mapping).length > 0) {
				query += `&sourceMapping=${encodeURIComponent(JSON.stringify(mapping))}`;
			}
			return api<SuwayomiImportResponse>(`/backups/import/suwayomi?${query}`, { method: 'POST', body: file });
		},
		onSuccess: (_result, vars) => {
			if (!vars.dryRun) qc.invalidateQueries();
		}
	});
}

export function useSourceSettings(sourceId: number, enabled = true) {
	return useQuery({
		queryKey: ['source-settings', sourceId],
		queryFn: () => api<SourceSettings>(`/sources/${sourceId}/settings`),
		enabled
	});
}

export function useSaveSourceSettings(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ edits, notify }: { edits: Record<string, unknown>; notify?: string[] }) => {
			// the notification declared by each changed setting is sent to the source one by one (repeated params, delivered in order by the server)
			const qs = (notify ?? []).map((n) => `notify=${encodeURIComponent(n)}`).join('&');
			return api<{ values: Record<string, unknown> }>(
				`/sources/${sourceId}/settings${qs ? `?${qs}` : ''}`,
				{ method: 'PUT', body: edits }
			);
		},
		onSuccess: (data) => {
			qc.setQueryData<SourceSettings>(['source-settings', sourceId], (cur) =>
				cur ? { ...cur, values: data.values } : cur
			);
		}
	});
}

/** Pressing a Button setting: sends its notification string to the source (without restarting the instance); the source does not respond when no action string is declared */
export function useNotifySource(sourceId: number) {
	return useMutation({
		mutationFn: (notification: string) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/notify`, { method: 'POST', body: { notification } })
	});
}

/** Map the refreshes declared by a settings change to query invalidations, mirroring the Aidoku app's refresh-* notifications */
export function invalidateForRefreshes(qc: QueryClient, sourceId: number, refreshes: string[]) {
	for (const target of new Set(refreshes)) {
		if (target === 'settings') void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] });
		if (target === 'filters') void qc.invalidateQueries({ queryKey: qk.sourceFilters(sourceId) });
		if (target === 'listings') void qc.invalidateQueries({ queryKey: qk.sourceListings(sourceId) });
		if (target === 'content') void qc.invalidateQueries({ queryKey: qk.sourceHome(sourceId) });
	}
}

/** Clear override values and reseed defaults from the package's static schema */
export function useResetSourceSettings(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () =>
			api<{ values: Record<string, unknown> }>(`/sources/${sourceId}/settings/reset`, {
				method: 'POST'
			}),
		onSuccess: (data) => {
			qc.setQueryData<SourceSettings>(['source-settings', sourceId], (cur) =>
				cur ? { ...cur, values: data.values } : cur
			);
		}
	});
}

/** Clear the source's login cookies and restart the instance (cover cache and downloads are untouched) */
export function useClearSourceCache(sourceId: number) {
	return useMutation({
		mutationFn: () => api<{ ok: boolean }>(`/sources/${sourceId}/cache/clear`, { method: 'POST' })
	});
}

/** Basic login: on success the server writes back username/password and the marker; refresh the settings values */
export function useBasicLogin(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ key, username, password }: { key: string; username: string; password: string }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/basic`, {
				method: 'POST',
				body: { key, username, password }
			}),
		onSuccess: () => void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	});
}

/** Web login: paste the login host's cookies in exchange for a logged-in state; refresh settings values after the server restarts the instance */
export function useWebLogin(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({
			key,
			cookies,
			localStorage
		}: {
			key: string;
			cookies: Record<string, string>;
			localStorage?: Record<string, string>;
		}) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/web`, {
				method: 'POST',
				body: { key, cookies, localStorage }
			}),
		onSuccess: () => void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	});
}

/** OAuth login: build the authorization URL (with pkce the server generates and stores the verifier, then appends the challenge); open in a new tab */
export function useOAuthLoginStart(sourceId: number) {
	return useMutation({
		mutationFn: (key: string) =>
			api<{ url: string }>(`/sources/${sourceId}/login/oauth/start`, {
				method: 'POST',
				body: { key }
			})
	});
}

/** OAuth login: parse the pasted callback URL to exchange for tokens or store the callback string; refresh settings values after the server restarts the instance */
export function useOAuthLoginComplete(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ key, url }: { key: string; url: string }) =>
			api<{ values: Record<string, unknown> }>(`/sources/${sourceId}/login/oauth/complete`, {
				method: 'POST',
				body: { key, url }
			}),
		onSuccess: () => void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	});
}

/** Source-level logout: clears <key> and its subkeys; when clearCookies is true the login cookies are cleared too */
export function useLoginLogout(sourceId: number) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ key, clearCookies }: { key: string; clearCookies: boolean }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/logout`, {
				method: 'POST',
				body: { key, clearCookies }
			}),
		onSuccess: () => void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	});
}

// a duplicate-name 409 needs to be turned into a Chinese message, so silent leaves the toast to the caller
export function useCreateCategory() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (name: string) =>
			api<Category>('/categories', { method: 'POST', body: { name }, silent: true }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.categories })
	});
}

export function useRenameCategory() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, name }: { id: number; name: string }) =>
			api<Category>(`/categories/${id}`, { method: 'PATCH', body: { name }, silent: true }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.categories })
	});
}

export function useDeleteCategory() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (id: number) => api(`/categories/${id}`, { method: 'DELETE' }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.categories })
	});
}

export function useReorderCategories() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (ids: number[]) =>
			api<Category[]>('/categories/reorder', { method: 'PATCH', body: { ids } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.categories })
	});
}

// ---- Filter groups ----

export function useFilterGroups() {
	return useQuery({ queryKey: qk.filterGroups, queryFn: () => api<FilterGroup[]>('/filter-groups') });
}

/** A group's rules feed the library query, so every mutation invalidates both the group list and the library */
function invalidateGroups(qc: QueryClient) {
	void qc.invalidateQueries({ queryKey: qk.filterGroups });
	void qc.invalidateQueries({ queryKey: qk.library });
}

// silent: a duplicate-name 409 is turned into a toast by the caller
export function useCreateFilterGroup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: { name: string; rules: FilterRule[] }) =>
			api<FilterGroup>('/filter-groups', { method: 'POST', body, silent: true }),
		onSuccess: () => invalidateGroups(qc)
	});
}

export function useUpdateFilterGroup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, ...body }: { id: number; name?: string; rules?: FilterRule[] }) =>
			api<FilterGroup>(`/filter-groups/${id}`, { method: 'PATCH', body, silent: true }),
		onSuccess: () => invalidateGroups(qc)
	});
}

export function useDeleteFilterGroup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (id: number) => api(`/filter-groups/${id}`, { method: 'DELETE' }),
		onSuccess: () => invalidateGroups(qc)
	});
}

export function useReorderFilterGroups() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (ids: number[]) =>
			api<FilterGroup[]>('/filter-groups/reorder', { method: 'PATCH', body: { ids } }),
		onSuccess: () => invalidateGroups(qc)
	});
}

// ---- Cross-source migration ----

export function useMigrateSources() {
	return useQuery({
		queryKey: ['migrate', 'sources'],
		queryFn: () => api<MigrationSource[]>('/migrate/sources')
	});
}

/** Title search of a single entry against one target source: imperative call; batch-match concurrency is owned by the page */
export function migrateLookup(mangaId: number, targetSourceId: number, query?: string) {
	const qs = new URLSearchParams({ mangaId: String(mangaId), targetSourceId: String(targetSourceId) });
	if (query) qs.set('query', query);
	return api<MigrationLookupResponse>(`/migrate/lookup?${qs}`, { silent: true });
}

export function useMigrate() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: MigrateRequest) => api<MigrateResponse>('/migrate', { method: 'POST', body }),
		// library/progress/history/categories may all change — full invalidation, same as backup restore
		onSuccess: () => qc.invalidateQueries()
	});
}
