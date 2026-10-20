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
	AidokuImportResult,
	BackupListResponse,
	BackupRestoreResult,
	Category,
	Chapter,
	ChapterPatchBody,
	ChapterProgress,
	DeepLinkResponse,
	Filter,
	FilterValue,
	GlobalSearchResponse,
	GlobalSearchStreamEvent,
	HomeLayout,
	HomeStreamEvent,
	LibraryPage,
	LibrarySort,
	LibrarySourceCount,
	Listing,
	Manga,
	MangaDetail,
	MangaPage,
	MangaUpdateStreamEvent,
	MigrateLookupResult,
	MigrateRequest,
	MigrateResponse,
	MigrateSource,
	PageData,
	PatchCategoryBody,
	PatchMangaBody,
	RepoSourceEntry,
	ScanOutcome,
	SearchStreamEvent,
	Source,
	SourceRepo,
	SourceSearchResult,
	SourceUpdate,
	SuwayomiImportResult,
	TriStateSelection,
	UpdateAllOutcome
} from './types';

export const queryClient = new QueryClient({
	defaultOptions: {
		queries: { staleTime: 30_000, retry: 1 }
	}
});

// query key 统一出口，跨页失效（如入库后刷新书库）按前缀对账
export const qk = {
	sources: ['sources'] as const,
	sourceRepos: ['source-repos'] as const,
	repoSources: ['source-repos', 'sources'] as const,
	categories: ['categories'] as const,
	library: ['library'] as const,
	manga: (id: number) => ['manga', id] as const,
	chapters: (mangaId: number) => ['manga', mangaId, 'chapters'] as const,
	sourceFilters: (sourceId: number) => ['sources', sourceId, 'filters'] as const,
	sourceListings: (sourceId: number) => ['sources', sourceId, 'listings'] as const,
	sourceHome: (sourceId: number) => ['sources', sourceId, 'home'] as const,
	backups: ['backups'] as const
};

const nextPage = (last: { hasNextPage: boolean }, all: { hasNextPage: boolean }[]) =>
	last.hasNextPage ? all.length + 1 : undefined;

/** 动态能力（filters/listings）不支持时服务端按约定 404，视为空列表而不是错误 */
async function emptyOn404<T>(p: Promise<T[]>): Promise<T[]> {
	try {
		return await p;
	} catch (e) {
		if (e instanceof ApiError && e.status === 404) return [];
		throw e;
	}
}

/** 主页能力不支持时按 null 处理，页面据此显示「该源没有主页」而不是报错 */
async function nullOn404<T>(p: Promise<T>): Promise<T | null> {
	try {
		return await p;
	} catch (e) {
		if (e instanceof ApiError && e.status === 404) return null;
		throw e;
	}
}

// ---- 查询 ----

export function useSources() {
	return useQuery({ queryKey: qk.sources, queryFn: () => api<Source[]>('/sources') });
}

/** 从源列表里取单个源；没有独立的 GET /sources/{id} 端点 */
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
	/** 布尔筛选三态（unread/downloaded/bookmarked 键）：1 包含 / -1 排除 */
	filters?: TriStateSelection;
	/** 迁移批量流程按源筛条目 */
	sourceId?: number;
	/** 内容分级三态（manga 级 1..=3 → 1 包含 / -1 排除） */
	ratings?: TriStateSelection;
	/** 源三态（源 id → 1 包含 / -1 排除） */
	sources?: TriStateSelection;
}

/** 三态拆成 API 的包含/排除两个逗号串；空集缺席 */
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

export function useLibrary(categoryId: number | undefined, browse: LibraryBrowse = {}) {
	const { search, sort, filters, sourceId, ratings, sources } = browse;
	const [filterInc, filterExc] = triParams(filters);
	const [ratingInc, ratingExc] = triParams(ratings);
	const [sourceInc, sourceExc] = triParams(sources);
	return useInfiniteQuery({
		queryKey: [...qk.library, { categoryId: categoryId ?? null, search, sort, filters, sourceId, ratings, sources }],
		queryFn: ({ pageParam }) => {
			const qs = new URLSearchParams({ page: String(pageParam) });
			if (categoryId !== undefined) qs.set('categoryId', String(categoryId));
			if (sourceId !== undefined) qs.set('sourceId', String(sourceId));
			if (search) qs.set('search', search);
			if (sort) qs.set('sort', sort);
			if (filterInc) qs.set('filter', filterInc);
			if (filterExc) qs.set('filterExclude', filterExc);
			if (ratingInc) qs.set('contentRating', ratingInc);
			if (ratingExc) qs.set('contentRatingExclude', ratingExc);
			if (sourceInc) qs.set('source', sourceInc);
			if (sourceExc) qs.set('sourceExclude', sourceExc);
			return api<LibraryPage>(`/library?${qs}`);
		},
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

export function useCategories() {
	return useQuery({ queryKey: qk.categories, queryFn: () => api<Category[]>('/categories') });
}

/** 书库各源条目数；queryKey 挂在 qk.library 下，入库/移出自动失效重取 */
export function useLibrarySourceCounts(enabled = true) {
	return useQuery({
		queryKey: [...qk.library, 'source-counts'],
		queryFn: () => api<LibrarySourceCount[]>('/library/source-counts'),
		enabled
	});
}

export function useSourceFilters(sourceId: number, enabled = true) {
	return useQuery({
		queryKey: qk.sourceFilters(sourceId),
		queryFn: () => emptyOn404(api<Filter[]>(`/sources/${sourceId}/filters`, { silent: true })),
		enabled
	});
}

export function useSourceListings(sourceId: number, enabled = true) {
	return useQuery({
		queryKey: qk.sourceListings(sourceId),
		queryFn: () => emptyOn404(api<Listing[]>(`/sources/${sourceId}/listings`, { silent: true })),
		enabled
	});
}

export function useSourceHome(sourceId: number, enabled = true) {
	const qc = useQueryClient();
	return useQuery({
		queryKey: qk.sourceHome(sourceId),
		queryFn: ({ signal }) =>
			nullOn404(
				apiStream<HomeLayout, HomeStreamEvent>(`/sources/${sourceId}/home`, {
					silent: true,
					signal,
					onEvent: (ev) => {
						// partial 到达即写缓存，页面边到边渲染；终态由 queryFn 返回后覆盖
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
					// partial 逐条追加到对应页；终态由 queryFn 返回后覆盖整页
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

/** 跨源聚合搜索：流式到达的每源结果按 sourceId 写进聚合缓存（完成一个上一个），
 * 终态返回后整体覆盖；页面按自己的源顺序取各源结果渲染 */
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
					const result: SourceSearchResult =
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

// ---- 变更 ----

export function useRegisterManga() {
	return useMutation({
		mutationFn: (body: { sourceId: number; manga: Manga }) =>
			apiStream<{ id: number }, MangaUpdateStreamEvent>('/manga', {
				method: 'POST',
				body,
				// 登记完成才拿得到内部 id，partial 没有可挂载的页面状态，忽略
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
		mutationFn: ({ id, ...body }: { id: number } & PatchMangaBody) =>
			api<MangaDetail>(`/manga/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d, { id }) => void qc.invalidateQueries({ queryKey: qk.manga(id) })
	});
}

export function usePatchCategory() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, ...body }: { id: number } & PatchCategoryBody) =>
			api<Category>(`/categories/${id}`, { method: 'PATCH', body }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.categories })
	});
}

// chapters 缓存按 mangaId 分键，而 PATCH 响应里只有 chapterId，调用方要自己带上 mangaId 才会失效章节列表
export function useUpdateChapter() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, mangaId: _mangaId, ...body }: { id: number; mangaId?: number } & ChapterPatchBody) =>
			api<ChapterProgress>(`/chapter/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d, { mangaId }) => {
			if (mangaId !== undefined) void qc.invalidateQueries({ queryKey: qk.chapters(mangaId) });
		}
	});
}

export function useBatchChapters() {
	return useMutation({
		mutationFn: (body: { chapterIds: number[] } & ChapterPatchBody) =>
			api<{ updated: number }>('/chapter/batch', { method: 'POST', body })
	});
}

export function useEnqueueDownloads() {
	return useMutation({
		mutationFn: (chapterIds: number[]) =>
			api<{ queued: number }>('/downloads', { method: 'POST', body: { chapterIds } })
	});
}

export function useScanSources() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () => api<ScanOutcome>('/sources/scan', { method: 'POST' }),
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

/** ids 是 source key 的有序数组，服务端要求与当前置顶集合一一对应，顺序没变别调 */
export function useReorderPinned() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (ids: string[]) =>
			api<string[]>('/sources/reorder-pinned', { method: 'POST', body: { ids } }),
		onSuccess: () => void qc.invalidateQueries({ queryKey: qk.sources })
	});
}

// ---- 源仓库 ----

export interface RepoBrowseFilter {
	/** 语言码多选，逗号拼接；空数组不过滤 */
	langs?: string[];
	query?: string;
	/** 内容分级多选，逗号拼接；undefined 不过滤，空串表示全不选 */
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
			// 显式区分：undefined 不过滤；空串表示一个分级都不选（服务端匹配空集）
			if (filter.contentRating !== undefined) qs.set('contentRating', filter.contentRating);
			const suffix = qs.size > 0 ? `?${qs}` : '';
			return api<RepoSourceEntry[]>(`/source-repos/sources${suffix}`);
		},
		enabled
	});
}

/** 安装/更新后已装列表与各过滤结果里的 installedVersion/updateVersion 都要重算 */
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
			api<SourceUpdate>(`/sources/${sourceId}/update`, { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(qc)
	});
}

export function useUpdateAllSources() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: () => api<UpdateAllOutcome>('/source-repos/update-all', { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(qc)
	});
}

// ---- manga 详情/章节查询 ----

export interface MangaQueryOpts {
	enabled?: boolean;
	/** 阅读器只把详情/章节当导航信息，拉不到不该弹全局错误提示 */
	silent?: boolean;
}

export function useManga(id: number, opts: MangaQueryOpts = {}) {
	return useQuery({
		queryKey: qk.manga(id),
		queryFn: () => api<MangaDetail>(`/manga/${id}`, { silent: opts.silent }),
		enabled: opts.enabled ?? true
	});
}

export function useMangaChapters(mangaId: number, opts: MangaQueryOpts = {}) {
	return useQuery({
		queryKey: qk.chapters(mangaId),
		queryFn: () => api<Chapter[]>(`/manga/${mangaId}/chapters`, { silent: opts.silent }),
		enabled: opts.enabled ?? true
	});
}

/** 整章页面在会话内不变，不重复回源 */
export function useChapterPages(chapterId: number) {
	return useQuery({
		queryKey: ['chapter', chapterId, 'pages'],
		queryFn: () => api<PageData[]>(`/chapter/${chapterId}/pages`),
		staleTime: Infinity
	});
}

// ?refresh 回源拉取耗时不可控，走 mutation 拿 pending 态，成功直接写缓存省一次重拉
export function useRefreshManga() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (id: number) =>
			apiStream<MangaDetail, MangaUpdateStreamEvent>(`/manga/${id}?refresh`, {
				onEvent: (ev) => {
					if (ev.type !== 'manga') return;
					// 详情快照渐进上屏；chapters 不进详情缓存，终态返回后整体覆盖
					const { chapters, ...detail } = ev.manga;
					qc.setQueryData<MangaDetail>(qk.manga(id), (cur) => (cur ? { ...cur, ...detail } : cur));
				}
			}),
		onSuccess: (data, id) => qc.setQueryData(qk.manga(id), data)
	});
}

/** 备选封面：501 是源没导出的正常分支，silent 交给页面按「没有备选」显示 */
export function useAlternateCovers(mangaId: number) {
	return useQuery({
		queryKey: ['manga', mangaId, 'alternate-covers'],
		queryFn: () => api<string[]>(`/manga/${mangaId}/covers/alternates`, { silent: true })
	});
}

// 响应直接写回详情缓存：cover 字段一变，coverUrl 的版本参数跟着变，浏览器才会重取图
export function useSelectCover() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ id, url }: { id: number; url: string }) =>
			api<MangaDetail>(`/manga/${id}/cover`, { method: 'PUT', body: { url } }),
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
			apiStream<Chapter[], MangaUpdateStreamEvent>(`/manga/${mangaId}/chapters?refresh`, {
				onEvent: (ev) => {
					if (ev.type !== 'manga' || !ev.manga.chapters) return;
					// 章节快照渐进上屏：源侧章节还没有库内 id/进度，负 id 占位，
					// 页面据此挡掉点读/选择/下载，终态返回后整体覆盖
					const partial: Chapter[] = ev.manga.chapters.map((c, i) => ({
						...c,
						id: -(i + 1),
						pageCount: null,
						lastPageRead: 0,
						isRead: false,
						isBookmarked: false,
						lastReadAt: null,
						isDownloaded: false
					}));
					qc.setQueryData(qk.chapters(mangaId), partial);
				}
			}),
		onSuccess: (data, mangaId) => qc.setQueryData(qk.chapters(mangaId), data)
	});
}

// ---- history / updates 分页 ----
import type { HistoryPage as HistoryPageData, SourceSettings, UpdateRecentPage } from './types';

export function useHistory() {
	return useInfiniteQuery({
		queryKey: ['history'],
		queryFn: ({ pageParam }) => api<HistoryPageData>(`/history?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

export function useUpdateRecent() {
	return useInfiniteQuery({
		queryKey: ['update-recent'],
		queryFn: ({ pageParam }) => api<UpdateRecentPage>(`/update/recent?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

// 下载列表状态由 /downloads/ws 驱动、页面自持，这里只放操作类 hooks
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

// 409 busy 由页面转成提示，silent 避免客户端再弹一条错误
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
		queryFn: () => api<Record<string, unknown>>('/settings')
	});
}

export function useSaveServerSettings() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: Record<string, unknown>) =>
			api<Record<string, unknown>>('/settings', { method: 'PUT', body }),
		onSuccess: (saved) => qc.setQueryData(['server-settings'], saved)
	});
}

/** 恢复备份：任意 section 都可能变，全量失效 */
export function useRestoreBackup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ doc, sections }: { doc: unknown; sections: string[] }) =>
			api<BackupRestoreResult>(`/backup?sections=${sections.join(',')}`, {
				method: 'POST',
				body: doc
			}),
		onSuccess: () => qc.invalidateQueries()
	});
}

export function useBackupFiles() {
	return useQuery({
		queryKey: qk.backups,
		queryFn: () => api<BackupListResponse>('/backups')
	});
}

/** 从服务器备份目录里的既有文件恢复：与上传恢复同一条导入路径，同样全量失效 */
export function useRestoreNamedBackup() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (name: string) =>
			api<BackupRestoreResult>('/backups/restore', { method: 'POST', body: { name } }),
		onSuccess: () => qc.invalidateQueries()
	});
}

/** 导入 Aidoku .aib：dryRun 只预览不写库，真导入才全量失效 */
export function useImportAidoku() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: ({ file, dryRun }: { file: File; dryRun: boolean }) =>
			api<AidokuImportResult>(`/import/aidoku?dryRun=${dryRun}`, { method: 'POST', body: file }),
		onSuccess: (_result, vars) => {
			if (!vars.dryRun) qc.invalidateQueries();
		}
	});
}

/** 导入 Suwayomi/Mihon 备份：mapping 只对 unmatched 组起作用，键是字符串形态的 Mihon 源 id */
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
			return api<SuwayomiImportResult>(`/import/suwayomi?${query}`, { method: 'POST', body: file });
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
			// 被改设置声明的 notification 逐个发给源（重复参数，服务端按序送达）
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

/** Button 设置的按下：把它的 notification 串发给源（不重启实例），无动作串时源侧不响应 */
export function useNotifySource(sourceId: number) {
	return useMutation({
		mutationFn: (notification: string) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/notify`, { method: 'POST', body: { notification } })
	});
}

/** 设置变更声明的 refreshes 映射成查询失效，对齐 Aidoku app 的 refresh-* 通知 */
export function invalidateForRefreshes(qc: QueryClient, sourceId: number, refreshes: string[]) {
	for (const target of new Set(refreshes)) {
		if (target === 'settings') void qc.invalidateQueries({ queryKey: ['source-settings', sourceId] });
		if (target === 'filters') void qc.invalidateQueries({ queryKey: qk.sourceFilters(sourceId) });
		if (target === 'listings') void qc.invalidateQueries({ queryKey: qk.sourceListings(sourceId) });
		if (target === 'content') void qc.invalidateQueries({ queryKey: qk.sourceHome(sourceId) });
	}
}

/** 清空覆盖值并按包内静态 schema 重新播种默认值 */
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

/** 清该源登录 cookie 并重启实例（不动封面缓存与下载） */
export function useClearSourceCache(sourceId: number) {
	return useMutation({
		mutationFn: () => api<{ ok: boolean }>(`/sources/${sourceId}/cache/clear`, { method: 'POST' })
	});
}

/** 基本登录：成功后会写回 username/password 与 marker，刷新设置值 */
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

/** 网页登录：粘贴登录 host 的 cookie 换取登录态，服务端重启实例后刷新设置值 */
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

/** OAuth 登录：构建授权 URL（pkce 时服务端生成 verifier 落库并追加 challenge），新标签打开 */
export function useOAuthLoginStart(sourceId: number) {
	return useMutation({
		mutationFn: (key: string) =>
			api<{ url: string }>(`/sources/${sourceId}/login/oauth/start`, {
				method: 'POST',
				body: { key }
			})
	});
}

/** OAuth 登录：解析粘贴的回调 URL，换令牌或存回调串，服务端重启实例后刷新设置值 */
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

/** 源级登出：清 <key> 及其子键；clearCookies 为真时连登录 cookie 一起清 */
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

// 重名 409 要转成中文提示，silent 交给调用方统一弹错
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

// ---- 跨源迁移 ----

export function useMigrateSources() {
	return useQuery({
		queryKey: ['migrate', 'sources'],
		queryFn: () => api<MigrateSource[]>('/migrate/sources')
	});
}

/** 单条目对一个目标源的标题搜索：命令式调用，批量匹配的并发由页面自持 */
export function migrateLookup(mangaId: number, targetSourceId: number, query?: string) {
	const qs = new URLSearchParams({ mangaId: String(mangaId), targetSourceId: String(targetSourceId) });
	if (query) qs.set('query', query);
	return api<MigrateLookupResult>(`/migrate/lookup?${qs}`, { silent: true });
}

export function useMigrate() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (body: MigrateRequest) => api<MigrateResponse>('/migrate', { method: 'POST', body }),
		// 库/进度/历史/分类都可能变，与备份恢复同级全量失效
		onSuccess: () => qc.invalidateQueries()
	});
}
