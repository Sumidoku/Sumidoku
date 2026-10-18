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
	Category,
	Chapter,
	ChapterPatchBody,
	ChapterProgress,
	Filter,
	FilterValue,
	HomeLayout,
	HomeStreamEvent,
	LibraryFilter,
	LibraryPage,
	LibrarySort,
	Listing,
	Manga,
	MangaDetail,
	MangaPage,
	PageData,
	PatchCategoryBody,
	PatchMangaBody,
	RepoSourceEntry,
	ScanOutcome,
	SearchStreamEvent,
	Source,
	SourceRepo,
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
	sourceHome: (sourceId: number) => ['sources', sourceId, 'home'] as const
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
	filter?: LibraryFilter[];
}

export function useLibrary(categoryId: number | undefined, browse: LibraryBrowse = {}) {
	const { search, sort, filter } = browse;
	return useInfiniteQuery({
		queryKey: [...qk.library, { categoryId: categoryId ?? null, search, sort, filter }],
		queryFn: ({ pageParam }) => {
			const qs = new URLSearchParams({ page: String(pageParam) });
			if (categoryId !== undefined) qs.set('categoryId', String(categoryId));
			if (search) qs.set('search', search);
			if (sort) qs.set('sort', sort);
			if (filter && filter.length > 0) qs.set('filter', filter.join(','));
			return api<LibraryPage>(`/library?${qs}`);
		},
		initialPageParam: 1,
		getNextPageParam: nextPage
	});
}

export function useCategories() {
	return useQuery({ queryKey: qk.categories, queryFn: () => api<Category[]>('/categories') });
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
			api<{ id: number }>('/manga', { method: 'POST', body })
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
			api<Source>(`/sources/${sourceId}/update`, { method: 'POST' }),
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
		mutationFn: (id: number) => api<MangaDetail>(`/manga/${id}?refresh`),
		onSuccess: (data, id) => qc.setQueryData(qk.manga(id), data)
	});
}

export function useRefreshChapters() {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: (mangaId: number) => api<Chapter[]>(`/manga/${mangaId}/chapters?refresh`),
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
		mutationFn: (edits: Record<string, unknown>) =>
			api<{ values: Record<string, unknown> }>(`/sources/${sourceId}/settings`, {
				method: 'PUT',
				body: edits
			}),
		onSuccess: (data) => {
			qc.setQueryData<SourceSettings>(['source-settings', sourceId], (cur) =>
				cur ? { ...cur, values: data.values } : cur
			);
		}
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
