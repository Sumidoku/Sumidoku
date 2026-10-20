import {
	QueryClient,
	createInfiniteQuery,
	createMutation,
	createQuery
} from '@tanstack/svelte-query';
import type { InfiniteData } from '@tanstack/svelte-query';
import { ApiError, api, apiStream } from './client';
import type {
	AidokuImportResult,
	BackupListResponse,
	BackupRestoreResult,
	BrowserLoginSession,
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
	LibraryFilter,
	LibraryPage,
	LibrarySort,
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
	UpdateAllOutcome,
	HistoryPage as HistoryPageData,
	SourceSettings,
	UpdateRecentPage
} from './types';

/** 响应式入参用取值函数传入（svelte-query v6 的 options 是函数，内部读取 getter 才会跟随依赖更新） */
type Get<T> = () => T;

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
	return createQuery(() => ({ queryKey: qk.sources, queryFn: () => api<Source[]>('/sources') }));
}

/** 从源列表里取单个源；没有独立的 GET /sources/{id} 端点 */
export function useSource(id: Get<number>) {
	return createQuery(() => ({
		queryKey: qk.sources,
		queryFn: () => api<Source[]>('/sources'),
		select: (list: Source[]) => list.find((s) => s.id === id())
	}));
}

export interface LibraryBrowse {
	search?: string;
	sort?: LibrarySort;
	filter?: LibraryFilter[];
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

export function useLibrary(categoryId: Get<number | undefined>, browse: Get<LibraryBrowse> = () => ({})) {
	return createInfiniteQuery(() => {
		const cat = categoryId();
		const { search, sort, filter, sourceId, ratings, sources } = browse();
		const [ratingInc, ratingExc] = triParams(ratings);
		const [sourceInc, sourceExc] = triParams(sources);
		return {
			queryKey: [
				...qk.library,
				{ categoryId: cat ?? null, search, sort, filter, sourceId, ratings, sources }
			],
			queryFn: ({ pageParam }: { pageParam: number }) => {
				const qs = new URLSearchParams({ page: String(pageParam) });
				if (cat !== undefined) qs.set('categoryId', String(cat));
				if (sourceId !== undefined) qs.set('sourceId', String(sourceId));
				if (search) qs.set('search', search);
				if (sort) qs.set('sort', sort);
				if (filter && filter.length > 0) qs.set('filter', filter.join(','));
				if (ratingInc) qs.set('contentRating', ratingInc);
				if (ratingExc) qs.set('contentRatingExclude', ratingExc);
				if (sourceInc) qs.set('source', sourceInc);
				if (sourceExc) qs.set('sourceExclude', sourceExc);
				return api<LibraryPage>(`/library?${qs}`);
			},
			initialPageParam: 1,
			getNextPageParam: nextPage
		};
	});
}

export function useCategories() {
	return createQuery(() => ({ queryKey: qk.categories, queryFn: () => api<Category[]>('/categories') }));
}

export function useSourceFilters(sourceId: Get<number>, enabled: Get<boolean> = () => true) {
	return createQuery(() => ({
		queryKey: qk.sourceFilters(sourceId()),
		queryFn: () => emptyOn404(api<Filter[]>(`/sources/${sourceId()}/filters`, { silent: true })),
		enabled: enabled()
	}));
}

export function useSourceListings(sourceId: Get<number>, enabled: Get<boolean> = () => true) {
	return createQuery(() => ({
		queryKey: qk.sourceListings(sourceId()),
		queryFn: () => emptyOn404(api<Listing[]>(`/sources/${sourceId()}/listings`, { silent: true })),
		enabled: enabled()
	}));
}

export function useSourceHome(sourceId: Get<number>, enabled: Get<boolean> = () => true) {
	return createQuery(() => ({
		queryKey: qk.sourceHome(sourceId()),
		queryFn: ({ signal }: { signal: AbortSignal }) =>
			nullOn404(
				apiStream<HomeLayout, HomeStreamEvent>(`/sources/${sourceId()}/home`, {
					silent: true,
					signal,
					onEvent: (ev) => {
						// partial 到达即写缓存，页面边到边渲染；终态由 queryFn 返回后覆盖
						queryClient.setQueryData<HomeLayout | null>(qk.sourceHome(sourceId()), (cur) => {
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
		enabled: enabled()
	}));
}

export interface SourceSearchRequest {
	query?: string;
	filters: FilterValue[];
}

export function useSourceSearch(
	sourceId: Get<number>,
	req: Get<SourceSearchRequest>,
	enabled: Get<boolean> = () => true
) {
	return createInfiniteQuery(() => {
		const sid = sourceId();
		const request = req();
		const queryKey = ['sources', sid, 'search', request] as const;
		return {
			queryKey,
			queryFn: ({ pageParam, signal }: { pageParam: number; signal: AbortSignal }) =>
				apiStream<MangaPage, SearchStreamEvent>(`/sources/${sid}/search`, {
					method: 'POST',
					body: { query: request.query || undefined, page: pageParam, filters: request.filters },
					signal,
					onEvent: (ev) => {
						if (ev.type !== 'manga') return;
						// partial 逐条追加到对应页；终态由 queryFn 返回后覆盖整页
						queryClient.setQueryData<InfiniteData<MangaPage, number>>(queryKey, (cur) => {
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
			enabled: enabled()
		};
	});
}

/** 跨源聚合搜索：流式到达的每源结果按 sourceId 写进聚合缓存（完成一个上一个），
 * 终态返回后整体覆盖；页面按自己的源顺序取各源结果渲染 */
export function useGlobalSearch(
	sourceIds: Get<number[]>,
	query: Get<string>,
	enabled: Get<boolean> = () => true
) {
	return createQuery(() => {
		const ids = sourceIds();
		const q = query();
		const queryKey = ['global-search', q, ids] as const;
		return {
			queryKey,
			queryFn: ({ signal }: { signal: AbortSignal }) =>
				apiStream<GlobalSearchResponse, GlobalSearchStreamEvent>('/search', {
					method: 'POST',
					body: { query: q || undefined, page: 1, sourceIds: ids },
					signal,
					onEvent: (ev) => {
						const result: SourceSearchResult =
							ev.type === 'source'
								? ev.source
								: { sourceId: ev.sourceId, entries: [], hasNextPage: false, error: ev.error };
						queryClient.setQueryData<GlobalSearchResponse>(queryKey, (cur) => {
							const rest = (cur?.sources ?? []).filter((s) => s.sourceId !== result.sourceId);
							return { sources: [...rest, result] };
						});
					}
				}),
			enabled: enabled()
		};
	});
}

export function useListing(
	sourceId: Get<number>,
	listingId: Get<string>,
	enabled: Get<boolean> = () => true
) {
	return createInfiniteQuery(() => ({
		queryKey: ['sources', sourceId(), 'listing', listingId()],
		queryFn: ({ pageParam }: { pageParam: number }) =>
			api<MangaPage>(
				`/sources/${sourceId()}/listings/${encodeURIComponent(listingId())}?page=${pageParam}`
			),
		initialPageParam: 1,
		getNextPageParam: nextPage,
		enabled: enabled()
	}));
}

// ---- 变更 ----

export function useRegisterManga() {
	return createMutation(() => ({
		mutationFn: (body: { sourceId: number; manga: Manga }) =>
			apiStream<{ id: number }, MangaUpdateStreamEvent>('/manga', {
				method: 'POST',
				body,
				// 登记完成才拿得到内部 id，partial 没有可挂载的页面状态，忽略
				onEvent: () => {}
			})
	}));
}

export function useDeepLink() {
	return createMutation(() => ({
		mutationFn: ({ sourceId, url }: { sourceId: number; url: string }) =>
			api<DeepLinkResponse>(`/sources/${sourceId}/deep-link`, { method: 'POST', body: { url } })
	}));
}

export function useAddToLibrary() {
	return createMutation(() => ({
		mutationFn: (mangaId: number) => api('/library', { method: 'POST', body: { mangaId } }),
		onSuccess: (_d: unknown, mangaId: number) => {
			void queryClient.invalidateQueries({ queryKey: qk.library });
			void queryClient.invalidateQueries({ queryKey: qk.manga(mangaId) });
		}
	}));
}

export function useRemoveFromLibrary() {
	return createMutation(() => ({
		mutationFn: (mangaId: number) => api(`/library/${mangaId}`, { method: 'DELETE' }),
		onSuccess: (_d: unknown, mangaId: number) => {
			void queryClient.invalidateQueries({ queryKey: qk.library });
			void queryClient.invalidateQueries({ queryKey: qk.manga(mangaId) });
		}
	}));
}

export function useSetMangaCategories() {
	return createMutation(() => ({
		mutationFn: (body: { mangaId: number; categoryIds: number[] }) =>
			api(`/manga/${body.mangaId}/categories`, {
				method: 'PUT',
				body: { categoryIds: body.categoryIds }
			}),
		onSuccess: (_d: unknown, body: { mangaId: number; categoryIds: number[] }) => {
			void queryClient.invalidateQueries({ queryKey: qk.manga(body.mangaId) });
			void queryClient.invalidateQueries({ queryKey: qk.categories });
			void queryClient.invalidateQueries({ queryKey: qk.library });
		}
	}));
}

export function usePatchManga() {
	return createMutation(() => ({
		mutationFn: ({ id, ...body }: { id: number } & PatchMangaBody) =>
			api<MangaDetail>(`/manga/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d: MangaDetail, vars: { id: number } & PatchMangaBody) =>
			void queryClient.invalidateQueries({ queryKey: qk.manga(vars.id) })
	}));
}

export function usePatchCategory() {
	return createMutation(() => ({
		mutationFn: ({ id, ...body }: { id: number } & PatchCategoryBody) =>
			api<Category>(`/categories/${id}`, { method: 'PATCH', body }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.categories })
	}));
}

// chapters 缓存按 mangaId 分键，而 PATCH 响应里只有 chapterId，调用方要自己带上 mangaId 才会失效章节列表
export function useUpdateChapter() {
	return createMutation(() => ({
		mutationFn: ({ id, mangaId: _mangaId, ...body }: { id: number; mangaId?: number } & ChapterPatchBody) =>
			api<ChapterProgress>(`/chapter/${id}`, { method: 'PATCH', body }),
		onSuccess: (_d: ChapterProgress, vars: { id: number; mangaId?: number } & ChapterPatchBody) => {
			if (vars.mangaId !== undefined)
				void queryClient.invalidateQueries({ queryKey: qk.chapters(vars.mangaId) });
		}
	}));
}

export function useBatchChapters() {
	return createMutation(() => ({
		mutationFn: (body: { chapterIds: number[] } & ChapterPatchBody) =>
			api<{ updated: number }>('/chapter/batch', { method: 'POST', body })
	}));
}

export function useEnqueueDownloads() {
	return createMutation(() => ({
		mutationFn: (chapterIds: number[]) =>
			api<{ queued: number }>('/downloads', { method: 'POST', body: { chapterIds } })
	}));
}

export function useScanSources() {
	return createMutation(() => ({
		mutationFn: () => api<ScanOutcome>('/sources/scan', { method: 'POST' }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.sources })
	}));
}

export function useDeleteSource() {
	return createMutation(() => ({
		mutationFn: (sourceId: number) => api(`/sources/${sourceId}`, { method: 'DELETE' }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.sources })
	}));
}

export function usePinSource() {
	return createMutation(() => ({
		mutationFn: ({ id, pinned }: { id: number; pinned: boolean }) =>
			api<Source>(`/sources/${id}`, { method: 'PATCH', body: { pinned } }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.sources })
	}));
}

/** ids 是 source key 的有序数组，服务端要求与当前置顶集合一一对应，顺序没变别调 */
export function useReorderPinned() {
	return createMutation(() => ({
		mutationFn: (ids: string[]) =>
			api<string[]>('/sources/reorder-pinned', { method: 'POST', body: { ids } }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.sources })
	}));
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
	return createQuery(() => ({
		queryKey: qk.sourceRepos,
		queryFn: () => api<SourceRepo[]>('/source-repos')
	}));
}

export function useRepoSources(filter: Get<RepoBrowseFilter>, enabled: Get<boolean> = () => true) {
	return createQuery(() => {
		const f = filter();
		return {
			queryKey: [...qk.repoSources, f],
			queryFn: () => {
				const qs = new URLSearchParams();
				if (f.langs?.length) qs.set('lang', f.langs.join(','));
				if (f.query) qs.set('query', f.query);
				// 显式区分：undefined 不过滤；空串表示一个分级都不选（服务端匹配空集）
				if (f.contentRating !== undefined) qs.set('contentRating', f.contentRating);
				const suffix = qs.size > 0 ? `?${qs}` : '';
				return api<RepoSourceEntry[]>(`/source-repos/sources${suffix}`);
			},
			enabled: enabled()
		};
	});
}

/** 安装/更新后已装列表与各过滤结果里的 installedVersion/updateVersion 都要重算 */
function invalidateSourceViews(qc: QueryClient) {
	void qc.invalidateQueries({ queryKey: qk.sources });
	void qc.invalidateQueries({ queryKey: qk.repoSources });
}

export function useAddSourceRepo() {
	return createMutation(() => ({
		mutationFn: (url: string) => api<SourceRepo>('/source-repos', { method: 'POST', body: { url } }),
		onSuccess: () => {
			void queryClient.invalidateQueries({ queryKey: qk.sourceRepos });
			void queryClient.invalidateQueries({ queryKey: qk.repoSources });
		}
	}));
}

export function useDeleteSourceRepo() {
	return createMutation(() => ({
		mutationFn: (repoId: number) => api(`/source-repos/${repoId}`, { method: 'DELETE' }),
		onSuccess: () => {
			void queryClient.invalidateQueries({ queryKey: qk.sourceRepos });
			void queryClient.invalidateQueries({ queryKey: qk.repoSources });
		}
	}));
}

export function useRefreshSourceRepo() {
	return createMutation(() => ({
		mutationFn: (repoId: number) =>
			api<SourceRepo>(`/source-repos/${repoId}/refresh`, { method: 'POST' }),
		onSettled: () => {
			void queryClient.invalidateQueries({ queryKey: qk.sourceRepos });
			void queryClient.invalidateQueries({ queryKey: qk.repoSources });
		}
	}));
}

export function useInstallRepoSource() {
	return createMutation(() => ({
		mutationFn: (body: { repoId: number; sourceKey: string }) =>
			api<Source>('/source-repos/install', { method: 'POST', body }),
		onSuccess: () => invalidateSourceViews(queryClient)
	}));
}

export function useUpdateSource() {
	return createMutation(() => ({
		mutationFn: (sourceId: number) =>
			api<SourceUpdate>(`/sources/${sourceId}/update`, { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(queryClient)
	}));
}

export function useUpdateAllSources() {
	return createMutation(() => ({
		mutationFn: () => api<UpdateAllOutcome>('/source-repos/update-all', { method: 'POST' }),
		onSuccess: () => invalidateSourceViews(queryClient)
	}));
}

// ---- manga 详情/章节查询 ----

export interface MangaQueryOpts {
	enabled?: boolean;
	/** 阅读器只把详情/章节当导航信息，拉不到不该弹全局错误提示 */
	silent?: boolean;
}

export function useManga(id: Get<number>, opts: Get<MangaQueryOpts> = () => ({})) {
	return createQuery(() => {
		const o = opts();
		return {
			queryKey: qk.manga(id()),
			queryFn: () => api<MangaDetail>(`/manga/${id()}`, { silent: o.silent }),
			enabled: o.enabled ?? true
		};
	});
}

export function useMangaChapters(mangaId: Get<number>, opts: Get<MangaQueryOpts> = () => ({})) {
	return createQuery(() => {
		const o = opts();
		return {
			queryKey: qk.chapters(mangaId()),
			queryFn: () => api<Chapter[]>(`/manga/${mangaId()}/chapters`, { silent: o.silent }),
			enabled: o.enabled ?? true
		};
	});
}

/** 整章页面在会话内不变，不重复回源 */
export function useChapterPages(chapterId: Get<number>) {
	return createQuery(() => ({
		queryKey: ['chapter', chapterId(), 'pages'],
		queryFn: () => api<PageData[]>(`/chapter/${chapterId()}/pages`),
		staleTime: Infinity
	}));
}

// ?refresh 回源拉取耗时不可控，走 mutation 拿 pending 态，成功直接写缓存省一次重拉
export function useRefreshManga() {
	return createMutation(() => ({
		mutationFn: (id: number) =>
			apiStream<MangaDetail, MangaUpdateStreamEvent>(`/manga/${id}?refresh`, {
				onEvent: (ev) => {
					if (ev.type !== 'manga') return;
					// 详情快照渐进上屏；chapters 不进详情缓存，终态返回后整体覆盖
					const { chapters, ...detail } = ev.manga;
					queryClient.setQueryData<MangaDetail>(qk.manga(id), (cur) => (cur ? { ...cur, ...detail } : cur));
				}
			}),
		onSuccess: (data: MangaDetail, id: number) => queryClient.setQueryData(qk.manga(id), data)
	}));
}

/** 备选封面：501 是源没导出的正常分支，silent 交给页面按「没有备选」显示 */
export function useAlternateCovers(mangaId: Get<number>) {
	return createQuery(() => ({
		queryKey: ['manga', mangaId(), 'alternate-covers'],
		queryFn: () => api<string[]>(`/manga/${mangaId()}/covers/alternates`, { silent: true })
	}));
}

// 响应直接写回详情缓存：cover 字段一变，coverUrl 的版本参数跟着变，浏览器才会重取图
export function useSelectCover() {
	return createMutation(() => ({
		mutationFn: ({ id, url }: { id: number; url: string }) =>
			api<MangaDetail>(`/manga/${id}/cover`, { method: 'PUT', body: { url } }),
		onSuccess: (data: MangaDetail, { id }: { id: number; url: string }) => {
			queryClient.setQueryData(qk.manga(id), data);
			void queryClient.invalidateQueries({ queryKey: qk.library });
			void queryClient.invalidateQueries({ queryKey: ['history'] });
			void queryClient.invalidateQueries({ queryKey: ['update-recent'] });
		}
	}));
}

export function useRefreshChapters() {
	return createMutation(() => ({
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
					queryClient.setQueryData(qk.chapters(mangaId), partial);
				}
			}),
		onSuccess: (data: Chapter[], mangaId: number) => queryClient.setQueryData(qk.chapters(mangaId), data)
	}));
}

// ---- history / updates 分页 ----

export function useHistory() {
	return createInfiniteQuery(() => ({
		queryKey: ['history'],
		queryFn: ({ pageParam }: { pageParam: number }) => api<HistoryPageData>(`/history?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	}));
}

export function useUpdateRecent() {
	return createInfiniteQuery(() => ({
		queryKey: ['update-recent'],
		queryFn: ({ pageParam }: { pageParam: number }) => api<UpdateRecentPage>(`/update/recent?page=${pageParam}`),
		initialPageParam: 1,
		getNextPageParam: nextPage
	}));
}

// 下载列表状态由 /downloads/ws 驱动、页面自持，这里只放操作类 hooks
export function useDownloadsStart() {
	return createMutation(() => ({
		mutationFn: () => api<{ running: boolean }>('/downloads/start', { method: 'POST' })
	}));
}

export function useDownloadsStop() {
	return createMutation(() => ({
		mutationFn: () => api<{ running: boolean }>('/downloads/stop', { method: 'POST' })
	}));
}

export function useDownloadsClear() {
	return createMutation(() => ({
		mutationFn: () => api<{ cleared: number }>('/downloads/clear', { method: 'POST' })
	}));
}

export function useDeleteDownload() {
	return createMutation(() => ({
		mutationFn: (chapterId: number) => api(`/downloads/${chapterId}`, { method: 'DELETE' })
	}));
}

// 409 busy 由页面转成提示，silent 避免客户端再弹一条错误
export function useFetchUpdate() {
	return createMutation(() => ({
		mutationFn: (categoryId?: number) =>
			api<{ started: boolean }>('/update/fetch', {
				method: 'POST',
				body: categoryId !== undefined ? { categoryId } : {},
				silent: true
			})
	}));
}

// ---- settings ----

export function useServerSettings() {
	return createQuery(() => ({
		queryKey: ['server-settings'],
		queryFn: () => api<Record<string, unknown>>('/settings')
	}));
}

export function useSaveServerSettings() {
	return createMutation(() => ({
		mutationFn: (body: Record<string, unknown>) =>
			api<Record<string, unknown>>('/settings', { method: 'PUT', body }),
		onSuccess: (saved: Record<string, unknown>) => queryClient.setQueryData(['server-settings'], saved)
	}));
}

/** 恢复备份：任意 section 都可能变，全量失效 */
export function useRestoreBackup() {
	return createMutation(() => ({
		mutationFn: ({ doc, sections }: { doc: unknown; sections: string[] }) =>
			api<BackupRestoreResult>(`/backup?sections=${sections.join(',')}`, {
				method: 'POST',
				body: doc
			}),
		onSuccess: () => queryClient.invalidateQueries()
	}));
}

export function useBackupFiles() {
	return createQuery(() => ({
		queryKey: qk.backups,
		queryFn: () => api<BackupListResponse>('/backups')
	}));
}

/** 从服务器备份目录里的既有文件恢复：与上传恢复同一条导入路径，同样全量失效 */
export function useRestoreNamedBackup() {
	return createMutation(() => ({
		mutationFn: (name: string) =>
			api<BackupRestoreResult>('/backups/restore', { method: 'POST', body: { name } }),
		onSuccess: () => queryClient.invalidateQueries()
	}));
}

/** 导入 Aidoku .aib：dryRun 只预览不写库，真导入才全量失效 */
export function useImportAidoku() {
	return createMutation(() => ({
		mutationFn: ({ file, dryRun }: { file: File; dryRun: boolean }) =>
			api<AidokuImportResult>(`/import/aidoku?dryRun=${dryRun}`, { method: 'POST', body: file }),
		onSuccess: (_result: AidokuImportResult, vars: { file: File; dryRun: boolean }) => {
			if (!vars.dryRun) void queryClient.invalidateQueries();
		}
	}));
}

/** 导入 Suwayomi/Mihon 备份：mapping 只对 unmatched 组起作用，键是字符串形态的 Mihon 源 id */
export function useImportSuwayomi() {
	return createMutation(() => ({
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
		onSuccess: (
			_result: SuwayomiImportResult,
			vars: { file: File; dryRun: boolean; mapping?: Record<string, string> }
		) => {
			if (!vars.dryRun) void queryClient.invalidateQueries();
		}
	}));
}

export function useSourceSettings(sourceId: Get<number>, enabled: Get<boolean> = () => true) {
	return createQuery(() => ({
		queryKey: ['source-settings', sourceId()],
		queryFn: () => api<SourceSettings>(`/sources/${sourceId()}/settings`),
		enabled: enabled()
	}));
}

export function useSaveSourceSettings(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ edits, notify }: { edits: Record<string, unknown>; notify?: string[] }) => {
			// 被改设置声明的 notification 逐个发给源（重复参数，服务端按序送达）
			const qs = (notify ?? []).map((n) => `notify=${encodeURIComponent(n)}`).join('&');
			return api<{ values: Record<string, unknown> }>(
				`/sources/${sourceId}/settings${qs ? `?${qs}` : ''}`,
				{ method: 'PUT', body: edits }
			);
		},
		onSuccess: (data: { values: Record<string, unknown> }) => {
			queryClient.setQueryData<SourceSettings>(['source-settings', sourceId], (cur) =>
				cur ? { ...cur, values: data.values } : cur
			);
		}
	}));
}

/** Button 设置的按下：把它的 notification 串发给源（不重启实例），无动作串时源侧不响应 */
export function useNotifySource(sourceId: number) {
	return createMutation(() => ({
		mutationFn: (notification: string) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/notify`, { method: 'POST', body: { notification } })
	}));
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
	return createMutation(() => ({
		mutationFn: () =>
			api<{ values: Record<string, unknown> }>(`/sources/${sourceId}/settings/reset`, {
				method: 'POST'
			}),
		onSuccess: (data: { values: Record<string, unknown> }) => {
			queryClient.setQueryData<SourceSettings>(['source-settings', sourceId], (cur) =>
				cur ? { ...cur, values: data.values } : cur
			);
		}
	}));
}

/** 清该源登录 cookie 并重启实例（不动封面缓存与下载） */
export function useClearSourceCache(sourceId: number) {
	return createMutation(() => ({
		mutationFn: () => api<{ ok: boolean }>(`/sources/${sourceId}/cache/clear`, { method: 'POST' })
	}));
}

/** 基本登录：成功后会写回 username/password 与 marker，刷新设置值 */
export function useBasicLogin(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ key, username, password }: { key: string; username: string; password: string }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/basic`, {
				method: 'POST',
				body: { key, username, password }
			}),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	}));
}

/** 网页登录：粘贴登录 host 的 cookie 换取登录态，服务端重启实例后刷新设置值 */
export function useWebLogin(sourceId: number) {
	return createMutation(() => ({
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
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	}));
}

/** sidecar 交互登录：开会话，服务端开浏览器 context 加载登录页 */
export function useStartBrowserLogin(sourceId: number) {
	return createMutation(() => ({
		mutationFn: (key: string) =>
			api<BrowserLoginSession>(`/sources/${sourceId}/login/web/browser`, {
				method: 'POST',
				body: { key }
			})
	}));
}

/** OAuth 登录：构建授权 URL（pkce 时服务端生成 verifier 落库并追加 challenge），新标签打开 */
export function useOAuthLoginStart(sourceId: number) {
	return createMutation(() => ({
		mutationFn: (key: string) =>
			api<{ url: string }>(`/sources/${sourceId}/login/oauth/start`, {
				method: 'POST',
				body: { key }
			})
	}));
}

/** OAuth 登录：解析粘贴的回调 URL，换令牌或存回调串，服务端重启实例后刷新设置值 */
export function useOAuthLoginComplete(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ key, url }: { key: string; url: string }) =>
			api<{ values: Record<string, unknown> }>(`/sources/${sourceId}/login/oauth/complete`, {
				method: 'POST',
				body: { key, url }
			}),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	}));
}

/** 完成交互登录：服务端收割 context 的 cookie 走 #112 管线，成功后刷新设置值 */
export function useCompleteBrowserLogin(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ key, session }: { key: string; session: string }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/web/browser/complete`, {
				method: 'POST',
				body: { key, session }
			}),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	}));
}

/** 取消交互登录：清理服务端浏览器 context；幂等，silent 交给调用方按需弹错 */
export function useCancelBrowserLogin(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ key, session }: { key: string; session: string }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/web/browser/cancel`, {
				method: 'POST',
				body: { key, session },
				silent: true
			})
	}));
}

/** 源级登出：清 <key> 及其子键；clearCookies 为真时连登录 cookie 一起清 */
export function useLoginLogout(sourceId: number) {
	return createMutation(() => ({
		mutationFn: ({ key, clearCookies }: { key: string; clearCookies: boolean }) =>
			api<{ ok: boolean }>(`/sources/${sourceId}/login/logout`, {
				method: 'POST',
				body: { key, clearCookies }
			}),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: ['source-settings', sourceId] })
	}));
}

// 重名 409 要转成提示，silent 交给调用方统一弹错
export function useCreateCategory() {
	return createMutation(() => ({
		mutationFn: (name: string) =>
			api<Category>('/categories', { method: 'POST', body: { name }, silent: true }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.categories })
	}));
}

export function useRenameCategory() {
	return createMutation(() => ({
		mutationFn: ({ id, name }: { id: number; name: string }) =>
			api<Category>(`/categories/${id}`, { method: 'PATCH', body: { name }, silent: true }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.categories })
	}));
}

export function useDeleteCategory() {
	return createMutation(() => ({
		mutationFn: (id: number) => api(`/categories/${id}`, { method: 'DELETE' }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.categories })
	}));
}

export function useReorderCategories() {
	return createMutation(() => ({
		mutationFn: (ids: number[]) =>
			api<Category[]>('/categories/reorder', { method: 'PATCH', body: { ids } }),
		onSuccess: () => void queryClient.invalidateQueries({ queryKey: qk.categories })
	}));
}

// ---- 跨源迁移 ----

export function useMigrateSources() {
	return createQuery(() => ({
		queryKey: ['migrate', 'sources'],
		queryFn: () => api<MigrateSource[]>('/migrate/sources')
	}));
}

/** 单条目对一个目标源的标题搜索：命令式调用，批量匹配的并发由页面自持 */
export function migrateLookup(mangaId: number, targetSourceId: number, query?: string) {
	const qs = new URLSearchParams({ mangaId: String(mangaId), targetSourceId: String(targetSourceId) });
	if (query) qs.set('query', query);
	return api<MigrateLookupResult>(`/migrate/lookup?${qs}`, { silent: true });
}

export function useMigrate() {
	return createMutation(() => ({
		mutationFn: (body: MigrateRequest) => api<MigrateResponse>('/migrate', { method: 'POST', body }),
		// 库/进度/历史/分类都可能变，与备份恢复同级全量失效
		onSuccess: () => queryClient.invalidateQueries()
	}));
}
