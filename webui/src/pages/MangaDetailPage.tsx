import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { BookOpen, ArrowDownUp, ArrowLeft, ArrowLeftRight, ExternalLink, EyeOff, Images, RefreshCw, Search, TriangleAlert } from 'lucide-react';
import { coverUrl } from '../api/client';
import {
	qk,
	useAddToLibrary,
	useBatchChapters,
	useCategories,
	useEnqueueDownloads,
	useManga,
	useMangaChapters,
	usePatchManga,
	useRefreshChapters,
	useRefreshManga,
	useRemoveFromLibrary,
	useSetMangaCategories,
	useSource,
	useSourceFilters,
	useUpdateChapter
} from '../api/hooks';
import type { Chapter, Filter, FilterValue } from '../api/types';
import { statusText } from '../lib/format';
import {
	applyPrefs,
	loadPrefs,
	nextTri,
	savePrefs
} from '../lib/chapterPrefs';
import type { ChapterPrefs, SortKey, Tri } from '../lib/chapterPrefs';
import {
	Button,
	Chip,
	Cover,
	EmptyState,
	IconButton,
	Input,
	Modal,
	pushToast,
	Select,
	Skeleton,
	Spinner,
	Switch
} from '../components';
import type { ChipColor } from '../components';
import { ChapterRow } from './manga/ChapterRow';
import type { ChapterDlState } from './manga/ChapterRow';
import { CoverPickerModal } from './manga/CoverPickerModal';
import { MigrateModal } from './manga/MigrateModal';
import { MoreMenu } from './manga/MoreMenu';
import './manga.css';

const STATUS_COLOR: Record<string, ChipColor> = {
	ongoing: 'accent',
	completed: 'success',
	hiatus: 'warning',
	cancelled: 'danger',
	unknown: 'default'
};

type ChapFilterKey = 'unread' | 'downloaded' | 'bookmarked' | 'locked';

const CHAP_FILTERS: { value: ChapFilterKey; label: string }[] = [
	{ value: 'unread', label: '未读' },
	{ value: 'downloaded', label: '已下载' },
	{ value: 'bookmarked', label: '书签' },
	{ value: 'locked', label: '锁定' }
];

const SORT_OPTIONS: { value: SortKey; label: string }[] = [
	{ value: 'source', label: '源序' },
	{ value: 'chapter', label: '章节号' },
	{ value: 'date', label: '上传日期' }
];

/** Tri-state chip colors: include=accent, exclude=danger, off=default */
function triColor(t: Tri): ChipColor {
	return t === 1 ? 'accent' : t === -1 ? 'danger' : 'default';
}

function triTitle(label: string, t: Tri): string {
	if (t === 1) return `当前只显示${label}章节，点击改为排除`;
	if (t === -1) return `当前排除${label}章节，点击关闭筛选`;
	return `当前不筛选${label}，点击改为只显示${label}`;
}

// Author/artist search capability mirrors Aidoku supportsAuthorSearch: the manifest declaration wins;
// only when undeclared (undefined) does it fall back to inferring from the filter list (a text filter with the same id suffices)
function creatorSearchable(
	filters: Filter[],
	flag: boolean | undefined,
	id: string
): boolean {
	return flag ?? hasTextFilter(filters, id);
}

function hasTextFilter(filters: Filter[], id: string): boolean {
	return filters.some((f) => f.type === 'text' && f.id === id);
}

// tag tapping mirrors Aidoku matchingGenreFilter: only multi-select/select filters with isGenre get an exact
// (case-sensitive) option-name match, and the submitted value is ids[i] ?? options[i]; no match means not tappable
function genreFilterValue(filters: Filter[], tag: string): FilterValue | null {
	for (const f of filters) {
		if ((f.type !== 'multi-select' && f.type !== 'select') || !f.is_genre) continue;
		const options = f.options ?? [];
		const i = options.indexOf(tag);
		if (i < 0) continue;
		// The submitted key matches the browse page's optionKey: ids when present, otherwise the option name
		const value = f.ids?.[i] ?? options[i];
		if (!value) continue;
		return f.type === 'multi-select'
			? { MultiSelect: { id: f.id, included: [value], excluded: [] } }
			: { Select: { id: f.id, value } };
	}
	return null;
}

export default function MangaDetailPage() {
	const { id: rawId } = useParams();
	const mangaId = Number(rawId);
	const navigate = useNavigate();
	const qc = useQueryClient();

	const mangaQuery = useManga(mangaId);
	const chaptersQuery = useMangaChapters(mangaId);
	const manga = mangaQuery.data ?? null;
	const chapters = useMemo(() => chaptersQuery.data ?? [], [chaptersQuery.data]);
	// The manifest declaration wins (Aidoku config?.supportsX ?? inferred); the filter list is the fallback when undeclared.
	// While manga has not arrived, 0 stands in: the source is not found and filters 404 into an empty list, so nothing is tappable
	const sourceQuery = useSource(manga?.sourceId ?? 0);
	// The local source has no filters concept (the endpoint 404s), so the query is not issued
	const isLocal = sourceQuery.data?.kind === 'local';
	const filtersQuery = useSourceFilters(manga?.sourceId ?? 0, !isLocal);

	const [descOpen, setDescOpen] = useState(false);
	const [removeOpen, setRemoveOpen] = useState(false);
	const [catOpen, setCatOpen] = useState(false);
	const [catSel, setCatSel] = useState<Set<number>>(new Set());

	const [search, setSearch] = useState('');
	const [migrateOpen, setMigrateOpen] = useState(false);
	const [coverPickerOpen, setCoverPickerOpen] = useState(false);
	// Sort/filter prefs persist per manga; loadedId keeps a manga switch from writing the previous entry's prefs under the new key
	const [prefs, setPrefs] = useState<ChapterPrefs>(() => loadPrefs(mangaId));
	const [loadedId, setLoadedId] = useState(mangaId);
	const [selecting, setSelecting] = useState(false);
	const [selected, setSelected] = useState<Set<number>>(new Set());

	// Enqueueing only records local instant feedback; download progress lives on the downloads page
	const [dlQueued, setDlQueued] = useState<Set<number>>(new Set());
	const [dlPending, setDlPending] = useState<Set<number>>(new Set());

	// Switching mangaId on the same route does not remount the component, so reload the prefs of the new manga
	useEffect(() => {
		setPrefs(loadPrefs(mangaId));
		setLoadedId(mangaId);
	}, [mangaId]);

	// Only write back when the prefs really belong to the current manga, avoiding transient cross-writes
	useEffect(() => {
		if (loadedId === mangaId && Number.isFinite(mangaId)) savePrefs(mangaId, prefs);
	}, [mangaId, loadedId, prefs]);

	const addToLibrary = useAddToLibrary();
	const removeFromLibrary = useRemoveFromLibrary();
	const setMangaCategories = useSetMangaCategories();
	const patchManga = usePatchManga();
	const refreshManga = useRefreshManga();
	const refreshChapters = useRefreshChapters();
	const updateChapter = useUpdateChapter();
	const batchChapters = useBatchChapters();
	const enqueueDownloads = useEnqueueDownloads();
	const categoriesQuery = useCategories();

	const unread = useMemo(() => chapters.filter((c) => !c.isRead).length, [chapters]);
	const continueCh =
		chapters.length === 0 ? null : (chapters.find((c) => !c.isRead) ?? chapters[chapters.length - 1]);

	// Languages/scanlation groups only feed the filter options, deduplicated across all chapters
	const languages = useMemo(() => {
		const set = new Set<string>();
		for (const c of chapters) if (c.language) set.add(c.language);
		return [...set];
	}, [chapters]);

	const scanlators = useMemo(() => {
		const set = new Set<string>();
		for (const c of chapters) for (const s of c.scanlators ?? []) set.add(s);
		return [...set];
	}, [chapters]);

	const visible = useMemo(() => {
		// Apply the persisted sort/filter first, then the title/number text search
		let list = applyPrefs(chapters, prefs);
		const q = search.trim().toLowerCase();
		if (q) {
			list = list.filter(
				(c) =>
					(c.title ?? '').toLowerCase().includes(q) ||
					(c.chapter_number !== null && String(c.chapter_number).includes(q))
			);
		}
		return list;
	}, [chapters, prefs, search]);

	const allSelected = visible.length > 0 && visible.every((c) => selected.has(c.id));
	const batchBusy = batchChapters.isPending || enqueueDownloads.isPending;

	function readTarget(c: Chapter): string {
		// Re-reading a finished chapter starts from page 1; only an unread chapter with progress resumes
		const resume = !c.isRead && c.lastPageRead > 0 ? c.lastPageRead : 0;
		return `/reader/${c.id}?manga=${mangaId}&page=${resume}`;
	}

	function activate(c: Chapter) {
		// Placeholder rows (negative ids) in the chapter refresh stream have no library id; reading/selecting waits for the terminal state
		if (c.id < 0) return;
		if (selecting) {
			setSelected((prev) => {
				const next = new Set(prev);
				if (next.has(c.id)) next.delete(c.id);
				else next.add(c.id);
				return next;
			});
		} else {
			navigate(readTarget(c));
		}
	}

	function exitSelect() {
		setSelecting(false);
		setSelected(new Set());
	}

	function toggleAll() {
		setSelected(allSelected ? new Set() : new Set(visible.filter((c) => c.id > 0).map((c) => c.id)));
	}

	function openCats() {
		setCatSel(new Set(manga?.categoryIds ?? []));
		setCatOpen(true);
	}

	function toggleCat(id: number) {
		setCatSel((prev) => {
			const next = new Set(prev);
			if (next.has(id)) next.delete(id);
			else next.add(id);
			return next;
		});
	}

	function saveCats() {
		setMangaCategories.mutate(
			{ mangaId, categoryIds: [...catSel] },
			{
				onSuccess: () => {
					pushToast('分类已保存', 'info');
					setCatOpen(false);
				}
			}
		);
	}

	function confirmRemove() {
		removeFromLibrary.mutate(mangaId, {
			onSuccess: () => {
				pushToast('已移出书库', 'info');
				setRemoveOpen(false);
			}
		});
	}

	function dlStateOf(c: Chapter): ChapterDlState {
		if (dlPending.has(c.id)) return 'pending';
		if (dlQueued.has(c.id)) return 'queued';
		return 'idle';
	}

	function downloadChapter(c: Chapter) {
		if (c.id < 0 || dlPending.has(c.id) || dlQueued.has(c.id) || c.isDownloaded) return;
		setDlPending((prev) => new Set(prev).add(c.id));
		enqueueDownloads.mutate([c.id], {
			onSuccess: () => {
				setDlQueued((prev) => new Set(prev).add(c.id));
				pushToast('已加入下载队列', 'info');
			},
			onSettled: () =>
				setDlPending((prev) => {
					const next = new Set(prev);
					next.delete(c.id);
					return next;
				})
		});
	}

	function batch(body: { isRead?: boolean; isBookmarked?: boolean }, done: (updated: number) => string) {
		batchChapters.mutate(
			{ chapterIds: [...selected], ...body },
			{
				onSuccess: (res) => {
					pushToast(done(res.updated), 'info');
					void qc.invalidateQueries({ queryKey: qk.chapters(mangaId) });
					void qc.invalidateQueries({ queryKey: qk.library });
					exitSelect();
				}
			}
		);
	}

	// Shared by the row menu and mark-previous: marking read follows Aidoku and only touches readable chapters
	// (unlocked or downloaded); marking unread wipes the reading trace server-side, progress included
	function applyRead(ids: number[], isRead: boolean, done: (updated: number) => string) {
		const targets = isRead
			? ids.filter((id) => {
					const c = chapters.find((x) => x.id === id);
					return c !== undefined && (!c.locked || !!c.isDownloaded);
				})
			: ids;
		if (targets.length === 0) {
			pushToast('锁定章节需先下载才能标为已读', 'info');
			return;
		}
		batchChapters.mutate(
			{ chapterIds: targets, isRead },
			{
				onSuccess: (res) => {
					pushToast(done(res.updated), 'info');
					void qc.invalidateQueries({ queryKey: qk.chapters(mangaId) });
					void qc.invalidateQueries({ queryKey: qk.library });
				}
			}
		);
	}

	function markRead(c: Chapter) {
		if (c.id < 0) return;
		applyRead([c.id], true, () => '已标记为已读');
	}

	function markUnread(c: Chapter) {
		if (c.id < 0) return;
		applyRead([c.id], false, () => '已标记为未读');
	}

	// Aidoku's MARK_PREVIOUS: everything displayed below the tapped row (display-relative, so with the
	// default descending sort that is every older chapter); placeholder refresh rows (negative ids) skip
	function markPrevious(c: Chapter, isRead: boolean) {
		const idx = visible.findIndex((x) => x.id === c.id);
		if (idx < 0) return;
		const ids = visible
			.slice(idx + 1)
			.filter((x) => x.id > 0)
			.map((x) => x.id);
		if (ids.length === 0) return;
		applyRead(ids, isRead, (n) => `已把下方 ${n} 章标为${isRead ? '已读' : '未读'}`);
	}

	function downloadSelected() {
		const ids = [...selected];
		enqueueDownloads.mutate(ids, {
			onSuccess: (res) => {
				pushToast(`已把 ${res.queued} 章加入下载队列`, 'info');
				setDlQueued((prev) => new Set([...prev, ...ids]));
				exitSelect();
			}
		});
	}

	if (mangaQuery.isPending) {
		return (
			<div className="page container">
				<div className="manga-skeleton">
					<Skeleton width={180} aspect="2 / 3" style={{ flex: 'none' }} />
					<div className="manga-skeleton__info">
						<Skeleton height={24} width="60%" />
						<Skeleton height={14} width="40%" />
						<Skeleton height={14} width="85%" />
						<Skeleton height={14} width="70%" />
					</div>
				</div>
				<div className="manga-skeleton-rows">
					{Array.from({ length: 10 }, (_, i) => (
						<Skeleton key={i} height={44} />
					))}
				</div>
			</div>
		);
	}

	if (mangaQuery.isError || !manga) {
		return (
			<div className="page container">
				<EmptyState
					icon={TriangleAlert}
					title="漫画加载失败"
					hint={mangaQuery.error?.message || '这部漫画不存在或已被移除'}
					action={
						<Button
							variant="primary"
							onClick={() => {
								void mangaQuery.refetch();
								void chaptersQuery.refetch();
							}}
						>
							重试
						</Button>
					}
				/>
			</div>
		);
	}

	// On the wire authors/artists/tags can be null (aidoku Option<Vec>), not empty arrays
	const authors = manga.authors ?? [];
	const artists = manga.artists ?? [];
	const tags = manga.tags ?? [];
	const filters = filtersQuery.data ?? [];
	const sourceConfig = sourceQuery.data?.config;
	// Deduplicated with order kept; a name appearing in both authors and artists searches as author (same as Aidoku's author search)
	const creators = [...new Set([...authors, ...artists])].map((name) => ({
		name,
		filterId: authors.includes(name) ? 'author' : 'artist'
	}));
	const statusColor = STATUS_COLOR[manga.status.toLowerCase()] ?? 'default';
	const allCats = categoriesQuery.data ?? [];

	// tag/author/artist search mirrors Aidoku: the query is empty and only filter values are sent, reusing the browse page's filters deep link
	function searchByFilters(values: FilterValue[]) {
		if (!manga?.sourceId) return;
		const q = encodeURIComponent(JSON.stringify(values));
		navigate(`/browse/${manga.sourceId}?filters=${q}`);
	}

	function goBack() {
		// history.state.idx counts SPA navigations: 0 means this page was entered directly (fresh load/deep link),
		// where -1 would leave the app — fall back to the library
		if (window.history.state?.idx > 0) navigate(-1);
		else navigate('/library');
	}

	return (
		<div className="page container">
			<div className="manga-hero">
				<div
					className="manga-hero__bg"
					style={{ backgroundImage: `url("${coverUrl(mangaId, manga.cover)}")` }}
				/>
				<div className="manga-hero__scrim" />
				<IconButton className="manga-hero__back" label="返回上一级" onClick={goBack}>
					<ArrowLeft size={18} />
				</IconButton>
				<div className="manga-hero__body">
					<div className="manga-hero__cover">
						{manga.content_rating === 'NSFW' && <span className="manga-hero__nsfw">NSFW</span>}
						<Cover src={coverUrl(mangaId, manga.cover)} title={manga.title} />
						<button
							type="button"
							className="manga-hero__cover-edit"
							onClick={() => setCoverPickerOpen(true)}
						>
							<Images size={13} />
							更换封面
						</button>
					</div>
					<div className="manga-hero__info">
						<h1 className="manga-hero__title">{manga.title}</h1>
						{creators.length > 0 && (
							<div className="manga-hero__creators">
								{creators.map((c, i) => {
									const flag =
										c.filterId === 'author'
											? sourceConfig?.supportsAuthorSearch
											: sourceConfig?.supportsArtistSearch;
									const searchable = creatorSearchable(filters, flag, c.filterId);
									return (
										<span key={c.name}>
											{i > 0 && ' / '}
											{searchable ? (
												<button
													type="button"
													className="manga-hero__creator"
													title={`搜索${c.filterId === 'author' ? '作者' : '画师'}：${c.name}`}
													onClick={() =>
														searchByFilters([{ Text: { id: c.filterId, value: c.name } }])
													}
												>
													{c.name}
												</button>
											) : (
												c.name
											)}
										</span>
									);
								})}
							</div>
						)}
						<div className="manga-hero__chips">
							<Chip color={statusColor}>{statusText(manga.status)}</Chip>
							{manga.url && (
								<a
									className="manga-hero__source"
									href={manga.url}
									target="_blank"
									rel="noreferrer"
									title="打开来源页面"
								>
									<ExternalLink size={13} />
									来源
								</a>
							)}
						</div>
						{tags.length > 0 && (
							<div className="manga-hero__tags">
							{tags.map((t) => {
								// Aidoku: the config.supportsTagSearch declaration wins (send Select "genre" with the raw tag as value);
								// only when undeclared does it fall back to an exact option match on isGenre filters
								const v = sourceConfig?.supportsTagSearch
									? { Select: { id: 'genre', value: t } }
									: genreFilterValue(filters, t);
								return (
										<Chip
											key={t}
											title={v ? `搜索标签：${t}` : undefined}
											onClick={v ? () => searchByFilters([v]) : undefined}
										>
											{t}
										</Chip>
									);
								})}
							</div>
						)}
						{manga.description && (
							<>
								<p className={descOpen ? 'manga-hero__desc' : 'manga-hero__desc manga-hero__desc--clamp'}>
									{manga.description}
								</p>
								<button
									type="button"
									className="manga-hero__desc-toggle"
									onClick={() => setDescOpen((v) => !v)}
								>
									{descOpen ? '收起' : '展开'}
								</button>
							</>
						)}
						<div className="manga-hero__actions">
							<Button
								variant="primary"
								disabled={!continueCh}
								onClick={() => continueCh && navigate(readTarget(continueCh))}
							>
								<BookOpen size={16} />
								继续阅读
							</Button>
							{manga.inLibrary ? (
								<>
									<Button variant="danger" onClick={() => setRemoveOpen(true)}>
										移出书库
									</Button>
									<Button onClick={openCats}>分类</Button>
								</>
							) : (
								<Button
									variant="primary"
									loading={addToLibrary.isPending}
									onClick={() =>
										addToLibrary.mutate(mangaId, {
											onSuccess: () => pushToast('已加入书库', 'info')
										})
									}
								>
									加入书库
								</Button>
							)}
							<IconButton
								label="刷新详情"
								disabled={refreshManga.isPending}
								onClick={() => refreshManga.mutate(mangaId)}
							>
								{refreshManga.isPending ? <Spinner size={16} /> : <RefreshCw size={17} />}
							</IconButton>
							<IconButton
								label={
									manga.incognito
										? '隐身阅读中：这部漫画的阅读不写入历史，点击关闭'
										: '隐身阅读：开启后这部漫画的阅读不写入历史'
								}
								className={manga.incognito ? 'icon-btn--active' : undefined}
								disabled={patchManga.isPending}
								onClick={() => patchManga.mutate({ id: mangaId, incognito: !manga.incognito })}
							>
								<EyeOff size={17} />
							</IconButton>
							<MoreMenu>
								{(closeMenu) => (
									<>
										<label className="manga-menu__row">
											<span>自动更新章节</span>
											<Switch
												checked={manga.update_strategy === 'Always'}
												disabled={patchManga.isPending}
												onChange={(on) =>
													patchManga.mutate({ id: mangaId, updateStrategy: on ? 'Always' : 'Never' })
												}
												label="自动更新章节"
											/>
										</label>
										<div className="manga-menu__hint">关闭后全局更新会跳过这部漫画</div>
										<button
											type="button"
											className="manga-menu__row"
											disabled={refreshChapters.isPending}
											onClick={() =>
												refreshChapters.mutate(mangaId, {
													onSuccess: () => {
														pushToast('章节列表已刷新', 'info');
														closeMenu();
													}
												})
											}
										>
											<span>刷新章节列表</span>
											{refreshChapters.isPending ? <Spinner size={14} /> : <RefreshCw size={14} />}
										</button>
										{manga.inLibrary && (
											<button
												type="button"
												className="manga-menu__row"
												onClick={() => {
													closeMenu();
													setMigrateOpen(true);
												}}
											>
												<span>切换图源</span>
												<ArrowLeftRight size={14} />
											</button>
										)}
									</>
								)}
							</MoreMenu>
						</div>
					</div>
				</div>
			</div>

			<div className="manga-chaps">
				<div className="manga-chaps__head">
					<h2>章节</h2>
					<span className="faint">
						共 {chapters.length} 章 · 未读 {unread}
					</span>
					{/* The local source has no filesystem watch: added/deleted chapters only surface via a manual rescan, so the button sits prominently */}
					{isLocal && (
						<Button
							size="sm"
							disabled={refreshChapters.isPending}
							onClick={() =>
								refreshChapters.mutate(mangaId, {
									onSuccess: () => pushToast('章节列表已刷新', 'info')
								})
							}
						>
							{refreshChapters.isPending ? <Spinner size={14} /> : <RefreshCw size={14} />}
							刷新
						</Button>
					)}
				</div>

				{selecting ? (
					<div className="manga-chaps__select-tools">
						<Button size="sm" onClick={toggleAll}>
							{allSelected ? '取消全选' : '全选'}
						</Button>
						<span className="manga-chaps__selected">已选 {selected.size} 章</span>
						<Button size="sm" onClick={exitSelect}>
							完成
						</Button>
					</div>
				) : (
					<div className="manga-chaps__tools">
						<Input
							icon={<Search size={15} />}
							type="search"
							placeholder="搜索章节标题或序号…"
							value={search}
							onChange={(e) => setSearch(e.currentTarget.value)}
						/>
						<Select
							value={prefs.sort}
							title="排序方式"
							aria-label="排序方式"
							onChange={(e) => {
								const v = e.currentTarget.value as SortKey;
								setPrefs((p) => ({ ...p, sort: v }));
							}}
						>
							{SORT_OPTIONS.map((o) => (
								<option key={o.value} value={o.value}>
									{o.label}
								</option>
							))}
						</Select>
						<Button
							size="sm"
							title={prefs.asc ? '当前升序（旧章在前），点击切换' : '当前降序（最新章在前），点击切换'}
							onClick={() => setPrefs((p) => ({ ...p, asc: !p.asc }))}
						>
							<ArrowDownUp size={14} />
							{prefs.asc ? '升序' : '降序'}
						</Button>
						<div className="manga-chaps__filters">
							{CHAP_FILTERS.map((f) => (
								<Chip
									key={f.value}
									color={triColor(prefs.filters[f.value])}
									className={prefs.filters[f.value] === -1 ? 'chip--exclude' : undefined}
									title={triTitle(f.label, prefs.filters[f.value])}
									onClick={() =>
										setPrefs((p) => ({
											...p,
											filters: { ...p.filters, [f.value]: nextTri(p.filters[f.value]) }
										}))
									}
								>
									{f.label}
								</Chip>
							))}
						</div>
						{languages.length > 1 && (
							<Select
								value={prefs.language ?? ''}
								title="语言"
								aria-label="语言"
								onChange={(e) => {
									const v = e.currentTarget.value;
									setPrefs((p) => ({ ...p, language: v || null }));
								}}
							>
								<option value="">全部语言</option>
								{languages.map((l) => (
									<option key={l} value={l}>
										{l}
									</option>
								))}
							</Select>
						)}
						{scanlators.length > 1 && (
							<Select
								value={prefs.scanlators[0] ?? ''}
								title="汉化组"
								aria-label="汉化组"
								onChange={(e) => {
									const v = e.currentTarget.value;
									setPrefs((p) => ({ ...p, scanlators: v ? [v] : [] }));
								}}
							>
								<option value="">全部汉化组</option>
								{scanlators.map((s) => (
									<option key={s} value={s}>
										{s}
									</option>
								))}
							</Select>
						)}
						{chapters.length > 0 && (
							<Button size="sm" onClick={() => setSelecting(true)}>
								选择
							</Button>
						)}
					</div>
				)}

				{chaptersQuery.isPending ? (
					<div className="manga-skeleton-rows">
						{Array.from({ length: 10 }, (_, i) => (
							<Skeleton key={i} height={44} />
						))}
					</div>
				) : chaptersQuery.isError ? (
					<EmptyState
						icon={TriangleAlert}
						title="章节列表加载失败"
						hint={chaptersQuery.error?.message}
						action={
							<Button variant="primary" onClick={() => void chaptersQuery.refetch()}>
								重试
							</Button>
						}
					/>
				) : chapters.length === 0 ? (
					<EmptyState icon={BookOpen} title="暂无章节" hint="可以在右上角菜单里刷新章节列表" />
				) : visible.length === 0 ? (
					<EmptyState icon={Search} title="没有符合条件的章节" hint="换个关键词或清掉筛选试试" />
				) : (
					<div className={selecting ? 'manga-chaps__list--selecting' : undefined}>
					{visible.map((c, i) => (
						<ChapterRow
							key={c.id}
							chapter={c}
							selecting={selecting}
							selected={selected.has(c.id)}
							onActivate={() => activate(c)}
							dlState={dlStateOf(c)}
							onDownload={() => downloadChapter(c)}
							hideDownload={isLocal}
							onToggleBookmark={() => {
								if (c.id < 0) return;
								updateChapter.mutate({ id: c.id, mangaId, isBookmarked: !c.isBookmarked });
							}}
							bookmarkPending={
								updateChapter.isPending && updateChapter.variables?.id === c.id
							}
							onMarkRead={() => markRead(c)}
							onMarkUnread={() => markUnread(c)}
							onMarkPreviousRead={i < visible.length - 1 ? () => markPrevious(c, true) : undefined}
							onMarkPreviousUnread={i < visible.length - 1 ? () => markPrevious(c, false) : undefined}
							markPending={batchChapters.isPending}
						/>
					))}
					</div>
				)}
			</div>

			{selecting && (
				<div className="manga-batch">
					<span className="manga-batch__count">已选 {selected.size}</span>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isRead: true }, (n) => `已把 ${n} 章标为已读`)}
					>
						标已读
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isRead: false }, (n) => `已把 ${n} 章标为未读`)}
					>
						标未读
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isBookmarked: true }, (n) => `已为 ${n} 章加书签`)}
					>
						加书签
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isBookmarked: false }, (n) => `已取消 ${n} 章的书签`)}
					>
						取消书签
					</Button>
					{!isLocal && (
						<Button
							size="sm"
							variant="primary"
							disabled={selected.size === 0 || batchBusy}
							onClick={downloadSelected}
						>
							下载
						</Button>
					)}
				</div>
			)}

			{migrateOpen && <MigrateModal manga={manga} onClose={() => setMigrateOpen(false)} />}

			{coverPickerOpen && <CoverPickerModal manga={manga} onClose={() => setCoverPickerOpen(false)} />}

			<Modal
				open={removeOpen}
				title="移出书库"				onClose={() => setRemoveOpen(false)}
				footer={
					<>
						<Button onClick={() => setRemoveOpen(false)}>取消</Button>
						<Button variant="danger" loading={removeFromLibrary.isPending} onClick={confirmRemove}>
							移出
						</Button>
					</>
				}
			>
				<p>确定把《{manga.title}》移出书库吗？</p>
			</Modal>

			<Modal
				open={catOpen}
				title="设置分类"
				onClose={() => setCatOpen(false)}
				footer={
					allCats.length > 0 ? (
						<Button variant="primary" loading={setMangaCategories.isPending} onClick={saveCats}>
							保存
						</Button>
					) : undefined
				}
			>
				{allCats.length === 0 ? (
					<p className="muted">还没有分类，可先在设置页创建。</p>
				) : (
					<div className="manga-cats">
						{allCats.map((c) => (
							<label key={c.id} className="manga-cats__item">
								<span>{c.name}</span>
								<Switch checked={catSel.has(c.id)} onChange={() => toggleCat(c.id)} label={c.name} />
							</label>
						))}
					</div>
				)}
			</Modal>
		</div>
	);
}
