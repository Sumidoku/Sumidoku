import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { BookOpen, ArrowDownUp, ArrowLeftRight, ExternalLink, EyeOff, Images, RefreshCw, Search, TriangleAlert } from 'lucide-react';
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
	useUpdateChapter
} from '../api/hooks';
import type { Chapter } from '../api/types';
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

/** 三态 chip 颜色：含=accent、排除=danger、关=default */
function triColor(t: Tri): ChipColor {
	return t === 1 ? 'accent' : t === -1 ? 'danger' : 'default';
}

function triTitle(label: string, t: Tri): string {
	if (t === 1) return `当前只显示${label}章节，点击改为排除`;
	if (t === -1) return `当前排除${label}章节，点击关闭筛选`;
	return `当前不筛选${label}，点击改为只显示${label}`;
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
	// 源 config 的 tag/artist/author 搜索能力位；manga 未到时以 0 占位查不到，自然不渲染链接
	const sourceQuery = useSource(manga?.sourceId ?? 0);

	const [descOpen, setDescOpen] = useState(false);
	const [removeOpen, setRemoveOpen] = useState(false);
	const [catOpen, setCatOpen] = useState(false);
	const [catSel, setCatSel] = useState<Set<number>>(new Set());

	const [search, setSearch] = useState('');
	const [migrateOpen, setMigrateOpen] = useState(false);
	const [coverPickerOpen, setCoverPickerOpen] = useState(false);
	// 排序/筛选设置按漫画持久化；loadedId 防止切漫画瞬间把上一部的设置写到新 key 下
	const [prefs, setPrefs] = useState<ChapterPrefs>(() => loadPrefs(mangaId));
	const [loadedId, setLoadedId] = useState(mangaId);
	const [selecting, setSelecting] = useState(false);
	const [selected, setSelected] = useState<Set<number>>(new Set());

	// 入队只记本地即时反馈，下载进度在下载页看
	const [dlQueued, setDlQueued] = useState<Set<number>>(new Set());
	const [dlPending, setDlPending] = useState<Set<number>>(new Set());

	// 同路由换 mangaId 时组件不重挂，切漫画要重载该部的设置
	useEffect(() => {
		setPrefs(loadPrefs(mangaId));
		setLoadedId(mangaId);
	}, [mangaId]);

	// 只在 prefs 确实属于当前漫画时回写，避免瞬态串写
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

	// 语言/扫图组只用于渲染筛选项，从全部章节里去重取值
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
		// 先应用持久化的排序/筛选，再做标题/序号文本搜索
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
		// 已读章节重读从第 1 页开始，未读有进度才续
		const resume = !c.isRead && c.lastPageRead > 0 ? c.lastPageRead : 0;
		return `/reader/${c.id}?manga=${mangaId}&page=${resume}`;
	}

	function activate(c: Chapter) {
		// 章节刷新流中的占位行（负 id）没有库内 id，点读/选择都等终态落地
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

	// wire 上 authors/artists/tags 可能是 null（aidoku Option<Vec>），不是空数组
	const authors = manga.authors ?? [];
	const artists = manga.artists ?? [];
	const tags = manga.tags ?? [];
	const sourceConfig = sourceQuery.data?.config;
	// 去重保序；同时在 authors/artists 出现的名字按 author 搜索（Aidoku 搜作者同款）
	const creators = [...new Set([...authors, ...artists])].map((name) => ({
		name,
		filterId: authors.includes(name) ? 'author' : 'artist'
	}));
	const statusColor = STATUS_COLOR[manga.status.toLowerCase()] ?? 'default';
	const allCats = categoriesQuery.data ?? [];

	// tag/artist/author 搜索：Aidoku 以 query 空 + Text 过滤器发起，这里复用浏览页的 filters 深链
	function searchByTextFilter(filterId: string, value: string) {
		if (!manga?.sourceId) return;
		const filters = encodeURIComponent(JSON.stringify([{ Text: { id: filterId, value } }]));
		navigate(`/browse/${manga.sourceId}?filters=${filters}`);
	}

	return (
		<div className="page container">
			<div className="manga-hero">
				<div
					className="manga-hero__bg"
					style={{ backgroundImage: `url("${coverUrl(mangaId, manga.cover)}")` }}
				/>
				<div className="manga-hero__scrim" />
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
									const searchable =
										c.filterId === 'author'
											? sourceConfig?.supportsAuthorSearch
											: sourceConfig?.supportsArtistSearch;
									return (
										<span key={c.name}>
											{i > 0 && ' / '}
											{searchable ? (
												<button
													type="button"
													className="manga-hero__creator"
													title={`搜索${c.filterId === 'author' ? '作者' : '画师'}：${c.name}`}
													onClick={() => searchByTextFilter(c.filterId, c.name)}
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
								{tags.map((t) => (
									<Chip
										key={t}
										title={sourceConfig?.supportsTagSearch ? `搜索标签：${t}` : undefined}
										onClick={
											sourceConfig?.supportsTagSearch
												? () => searchByTextFilter('tag', t)
												: undefined
										}
									>
										{t}
									</Chip>
								))}
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
						{visible.map((c) => (
							<ChapterRow
								key={c.id}
								chapter={c}
								selecting={selecting}
								selected={selected.has(c.id)}
								onActivate={() => activate(c)}
								dlState={dlStateOf(c)}
								onDownload={() => downloadChapter(c)}
								onToggleBookmark={() => {
									if (c.id < 0) return;
									updateChapter.mutate({ id: c.id, mangaId, isBookmarked: !c.isBookmarked });
								}}
								bookmarkPending={
									updateChapter.isPending && updateChapter.variables?.id === c.id
								}
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
					<Button
						size="sm"
						variant="primary"
						disabled={selected.size === 0 || batchBusy}
						onClick={downloadSelected}
					>
						下载
					</Button>
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
