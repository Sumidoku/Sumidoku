import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { BookOpen, ArrowDownUp, ArrowLeft, ArrowLeftRight, Download as DownloadIcon, ExternalLink, EyeOff, Images, RefreshCw, Search, TriangleAlert } from 'lucide-react';
import { api, coverUrl } from '../api/client';
import { useDocumentTitle } from '../components/DocumentTitle';
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
import type { ChapterWithDownload, Download, FilterValue, SourceFilter } from '../api/types';
import type { DownloadChapterEvent, EventSnapshot } from '../api/protocol';
import { sse } from '../api/sse';
import { createMangaDownloadEventHandlers, downloadActivitySnapshot, mergeDownloadResponse } from '../lib/downloadEvents';
import type { MangaDownloadActivity } from '../lib/downloadEvents';
import { selectDownloadChapterIds } from '../lib/downloadRules';
import type { DownloadRule } from '../lib/downloadRules';
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

const CHAP_FILTERS: { value: ChapFilterKey; labelKey: string }[] = [
	{ value: 'unread', labelKey: 'mangaDetail.filterUnread' },
	{ value: 'downloaded', labelKey: 'mangaDetail.filterDownloaded' },
	{ value: 'bookmarked', labelKey: 'mangaDetail.filterBookmarked' },
	{ value: 'locked', labelKey: 'mangaDetail.filterLocked' }
];

const SORT_OPTIONS: { value: SortKey; labelKey: string }[] = [
	{ value: 'source', labelKey: 'mangaDetail.sortSource' },
	{ value: 'chapter', labelKey: 'mangaDetail.sortChapter' },
	{ value: 'date', labelKey: 'mangaDetail.sortDate' }
];

// Author/artist search capability mirrors Aidoku supportsAuthorSearch: the manifest declaration wins;
// only when undeclared (undefined) does it fall back to inferring from the filter list (a text filter with the same id suffices)
function creatorSearchable(
	filters: SourceFilter[],
	flag: boolean | undefined,
	id: string
): boolean {
	return flag ?? hasTextFilter(filters, id);
}

function hasTextFilter(filters: SourceFilter[], id: string): boolean {
	return filters.some((f) => f.type === 'text' && f.id === id);
}

// tag tapping mirrors Aidoku matchingGenreFilter: only multi-select/select filters with isGenre get an exact
// (case-sensitive) option-name match, and the submitted value is ids[i] ?? options[i]; no match means not tappable
function genreFilterValue(filters: SourceFilter[], tag: string): FilterValue | null {
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
	const { t } = useTranslation();
	const { id: rawId } = useParams();
	const mangaId = Number(rawId);
	const navigate = useNavigate();
	const qc = useQueryClient();

	const mangaQuery = useManga(mangaId);
	const chaptersQuery = useMangaChapters(mangaId);
	const manga = mangaQuery.data ?? null;
	useDocumentTitle(manga?.title);
	const chapters = useMemo(() => chaptersQuery.data ?? [], [chaptersQuery.data]);
	// The manifest declaration wins (Aidoku config?.supportsX ?? inferred); the filter list is the fallback when undeclared.
	// While manga has not arrived, 0 is only a placeholder and cannot match a real source
	const sourceQuery = useSource(manga?.sourceId ?? 0);
	// The local source has no filters concept (the endpoint returns 501 unsupported), so the query is not issued
	const isLocal = sourceQuery.data?.kind === 'local';
	const filtersQuery = useSourceFilters(manga?.sourceId ?? 0, !isLocal);

	const [descOpen, setDescOpen] = useState(false);
	const [removeOpen, setRemoveOpen] = useState(false);
	const [catOpen, setCatOpen] = useState(false);
	const [catSel, setCatSel] = useState<Set<number>>(new Set());

	const [search, setSearch] = useState('');
	const [migrateOpen, setMigrateOpen] = useState(false);
	const [coverPickerOpen, setCoverPickerOpen] = useState(false);
	const [downloadRulesOpen, setDownloadRulesOpen] = useState(false);
	const [downloadRule, setDownloadRule] = useState<DownloadRule>('unread');
	const [downloadLatest, setDownloadLatest] = useState('10');
	const [downloadRuleQueued, setDownloadRuleQueued] = useState<number | null>(null);
	// Sort/filter prefs persist per manga; loadedId keeps a manga switch from writing the previous entry's prefs under the new key
	const [prefs, setPrefs] = useState<ChapterPrefs>(() => loadPrefs(mangaId));
	const [loadedId, setLoadedId] = useState(mangaId);
	const [selecting, setSelecting] = useState(false);
	const [selected, setSelected] = useState<Set<number>>(new Set());

	const [dlActivity, setDlActivity] = useState<Map<number, MangaDownloadActivity>>(new Map());
	const [dlPending, setDlPending] = useState<Set<number>>(new Set());

	useEffect(() => {
		let active = true;
		let receivedSnapshot = false;
		const pendingEvents = new Map<number, DownloadChapterEvent>();
		const handlers = createMangaDownloadEventHandlers(qk.chapters(mangaId), qk.library, {
			setChapters: (updater) => {
				qc.setQueryData<ChapterWithDownload[]>(qk.chapters(mangaId), updater);
			},
			invalidate: (queryKey) => {
				void qc.invalidateQueries({ queryKey });
			},
			setActivity: (chapterId, activity) => {
				setDlActivity((prev) => {
					const next = new Map(prev);
					if (activity) next.set(chapterId, activity);
					else next.delete(chapterId);
					return next;
				});
			},
			syncActivity: (downloads) => {
				setDlActivity(downloadActivitySnapshot(downloads, mangaId));
			}
		});
		const unsubDownload = sse.on('download', (msg) => {
			const event = msg as DownloadChapterEvent;
			pendingEvents.set(event.chapterId, event);
			handlers.onDownload(event);
		});
		const unsubSnapshot = sse.on('snapshot', (msg) => {
			receivedSnapshot = true;
			pendingEvents.clear();
			handlers.onSnapshot((msg as EventSnapshot).downloads);
		});
		// Only a real snapshot replaces the REST seed; ordinary events merge into it instead of suppressing it.
		void api<Download[]>('/downloads')
			.then((downloads) => {
				if (active && !receivedSnapshot) {
					handlers.onSnapshot(mergeDownloadResponse([], downloads, pendingEvents));
					pendingEvents.clear();
				}
			})
			.catch(() => undefined);
		return () => {
			active = false;
			unsubDownload();
			unsubSnapshot();
		};
	}, [mangaId, qc]);

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
	const downloadRuleIds = useMemo(
		() => selectDownloadChapterIds(chapters, downloadRule, Number(downloadLatest)),
		[chapters, downloadRule, downloadLatest]
	);

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

	const continueCh = useMemo(() => {
		const readable = (c: ChapterWithDownload) => !c.locked || !!c.isDownloaded;
		const resume = visible
			.filter((c) => readable(c) && !c.isRead && (c.lastReadAt ?? 0) > 0)
			.reduce<ChapterWithDownload | null>(
				(best, c) => ((c.lastReadAt ?? 0) > (best?.lastReadAt ?? 0) ? c : best),
				null
			);
		if (resume) return resume;
		const order = prefs.asc ? visible : [...visible].reverse();
		return order.find((c) => readable(c) && !c.isRead) ?? order[0] ?? null;
	}, [visible, prefs.asc]);

	const allSelected = visible.length > 0 && visible.every((c) => selected.has(c.id));
	const batchBusy = batchChapters.isPending || enqueueDownloads.isPending;

	function triTitle(label: string, tri: Tri): string {
		if (tri === 1) return t('mangaDetail.triOnly', { label });
		if (tri === -1) return t('mangaDetail.triExclude', { label });
		return t('mangaDetail.triOff', { label });
	}

	function readTarget(c: ChapterWithDownload): string {
		// Re-reading a finished chapter starts from page 1; only an unread chapter with progress resumes
		const resume = !c.isRead && c.lastPageRead > 0 ? c.lastPageRead : 0;
		return `/reader/${c.id}?manga=${mangaId}&page=${resume}`;
	}

	function activate(c: ChapterWithDownload) {
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
					pushToast(t('mangaDetail.catsSaved'), 'info');
					setCatOpen(false);
				}
			}
		);
	}

	function confirmRemove() {
		removeFromLibrary.mutate(mangaId, {
			onSuccess: () => {
				pushToast(t('mangaDetail.removedFromLibrary'), 'info');
				setRemoveOpen(false);
			}
		});
	}

	function dlStateOf(c: ChapterWithDownload): ChapterDlState {
		if (dlPending.has(c.id)) return 'pending';
		const activity = dlActivity.get(c.id);
		if (activity?.status === 'downloading') return 'downloading';
		if (activity) return 'queued';
		return 'idle';
	}

	function downloadChapter(c: ChapterWithDownload) {
		if (c.id < 0 || dlPending.has(c.id) || dlActivity.has(c.id) || c.isDownloaded) return;
		setDlPending((prev) => new Set(prev).add(c.id));
		enqueueDownloads.mutate([c.id], {
			onSuccess: () => pushToast(t('mangaDetail.downloadQueued'), 'info'),
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
			pushToast(t('mangaDetail.lockedNeedDownload'), 'info');
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

	function markRead(c: ChapterWithDownload) {
		if (c.id < 0) return;
		applyRead([c.id], true, (n) => t('mangaDetail.markedRead', { count: n }));
	}

	function markUnread(c: ChapterWithDownload) {
		if (c.id < 0) return;
		applyRead([c.id], false, (n) => t('mangaDetail.markedUnread', { count: n }));
	}

	// Aidoku's MARK_PREVIOUS: everything displayed below the tapped row (display-relative, so with the
	// default descending sort that is every older chapter); placeholder refresh rows (negative ids) skip
	function markPrevious(c: ChapterWithDownload, isRead: boolean) {
		const idx = visible.findIndex((x) => x.id === c.id);
		if (idx < 0) return;
		const ids = visible
			.slice(idx + 1)
			.filter((x) => x.id > 0)
			.map((x) => x.id);
		if (ids.length === 0) return;
		applyRead(ids, isRead, (n) =>
			isRead
				? t('mangaDetail.markedPreviousRead', { count: n })
				: t('mangaDetail.markedPreviousUnread', { count: n })
		);
	}

	function downloadSelected() {
		const ids = [...selected];
		enqueueDownloads.mutate(ids, {
			onSuccess: (res) => {
				pushToast(t('mangaDetail.queuedChapters', { count: res.queued.length }), 'info');
				exitSelect();
			}
		});
	}

	function downloadByRule() {
		const ids = downloadRuleIds;
		if (ids.length === 0) return;
		setDownloadRuleQueued(null);
		enqueueDownloads.mutate(ids, {
			onSuccess: (res) => setDownloadRuleQueued(res.queued.length)
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
					title={t('mangaDetail.loadFailed')}
					hint={mangaQuery.error?.message || t('mangaDetail.loadFailedHint')}
					action={
						<Button
							variant="primary"
							onClick={() => {
								void mangaQuery.refetch();
								void chaptersQuery.refetch();
							}}
						>
							{t('mangaDetail.retry')}
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
				<IconButton className="manga-hero__back" label={t('mangaDetail.back')} onClick={goBack}>
					<ArrowLeft size={18} />
				</IconButton>
				<div className="manga-hero__body">
					<div className="manga-hero__cover">
						{manga.content_rating === 'NSFW' && <span className="manga-hero__nsfw">NSFW</span>}
						<Cover src={coverUrl(mangaId, manga.cover)} title={manga.title} />
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
													title={t('mangaDetail.searchCreator', {
														kind: c.filterId === 'author' ? t('mangaDetail.author') : t('mangaDetail.artist'),
														name: c.name
													})}
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
									title={t('mangaDetail.openSourcePage')}
								>
									<ExternalLink size={13} />
									{t('mangaDetail.source')}
								</a>
							)}
						</div>
						{tags.length > 0 && (
							<div className="manga-hero__tags">
							{tags.map((tag) => {
								// Aidoku: the config.supportsTagSearch declaration wins (send Select "genre" with the raw tag as value);
								// only when undeclared does it fall back to an exact option match on isGenre filters
								const v = sourceConfig?.supportsTagSearch
									? { Select: { id: 'genre', value: tag } }
									: genreFilterValue(filters, tag);
								return (
										<Chip
											key={tag}
											title={v ? t('mangaDetail.searchTag', { tag }) : undefined}
											onClick={v ? () => searchByFilters([v]) : undefined}
										>
											{tag}
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
									{descOpen ? t('mangaDetail.collapse') : t('mangaDetail.expand')}
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
								{t('mangaDetail.continueReading')}
							</Button>
							{manga.inLibrary ? (
								<>
									<Button variant="danger" onClick={() => setRemoveOpen(true)}>
										{t('mangaDetail.removeFromLibrary')}
									</Button>
									<Button onClick={openCats}>{t('mangaDetail.categories')}</Button>
								</>
							) : (
								<Button
									variant="primary"
									loading={addToLibrary.isPending}
									onClick={() =>
										addToLibrary.mutate(mangaId, {
											onSuccess: () => pushToast(t('mangaDetail.addedToLibrary'), 'info')
										})
									}
								>
									{t('mangaDetail.addToLibrary')}
								</Button>
							)}
							<IconButton
								label={t('mangaDetail.refreshDetails')}
								disabled={refreshManga.isPending}
								onClick={() => refreshManga.mutate(mangaId)}
							>
								{refreshManga.isPending ? <Spinner size={16} /> : <RefreshCw size={17} />}
							</IconButton>
							<IconButton
								label={
									manga.incognito
										? t('mangaDetail.incognitoOn')
										: t('mangaDetail.incognitoOff')
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
											<span>{t('mangaDetail.autoUpdate')}</span>
											<Switch
												checked={manga.update_strategy === 'Always'}
												disabled={patchManga.isPending}
												onChange={(on) =>
													patchManga.mutate({ id: mangaId, updateStrategy: on ? 'Always' : 'Never' })
												}
												label={t('mangaDetail.autoUpdate')}
											/>
										</label>
										<div className="manga-menu__hint">{t('mangaDetail.autoUpdateHint')}</div>
										<button
											type="button"
											className="manga-menu__row"
											disabled={refreshChapters.isPending}
											onClick={() =>
												refreshChapters.mutate(mangaId, {
													onSuccess: () => {
														pushToast(t('mangaDetail.chaptersRefreshed'), 'info');
														closeMenu();
													}
												})
											}
										>
											<span>{t('mangaDetail.refreshChapters')}</span>
											{refreshChapters.isPending ? <Spinner size={14} /> : <RefreshCw size={14} />}
										</button>
										<button
											type="button"
											className="manga-menu__row"
											onClick={() => {
												closeMenu();
												setCoverPickerOpen(true);
											}}
										>
											<span>{t('mangaDetail.changeCover')}</span>
											<Images size={14} />
										</button>
										{!isLocal && chapters.length > 0 && (
											<button
												type="button"
												className="manga-menu__row"
												onClick={() => {
													closeMenu();
													setDownloadRuleQueued(null);
													setDownloadRulesOpen(true);
												}}
											>
												<span>{t('mangaDetail.downloadRulesTitle')}</span>
												<DownloadIcon size={14} />
											</button>
										)}
										{manga.inLibrary && (
											<button
												type="button"
												className="manga-menu__row"
												onClick={() => {
													closeMenu();
													setMigrateOpen(true);
												}}
											>
												<span>{t('mangaDetail.migrate')}</span>
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
					<h2>{t('mangaDetail.chapters')}</h2>
					<span className="faint">
						{t('mangaDetail.chapterStats', { total: chapters.length, unread, count: chapters.length })}
					</span>
					{/* The local source has no filesystem watch: added/deleted chapters only surface via a manual rescan, so the button sits prominently */}
					{isLocal && (
						<Button
							size="sm"
							disabled={refreshChapters.isPending}
							onClick={() =>
								refreshChapters.mutate(mangaId, {
									onSuccess: () => pushToast(t('mangaDetail.chaptersRefreshed'), 'info')
								})
							}
						>
							{refreshChapters.isPending ? <Spinner size={14} /> : <RefreshCw size={14} />}
							{t('mangaDetail.refresh')}
						</Button>
					)}
				</div>

				{selecting ? (
					<div className="manga-chaps__select-tools">
						<Button size="sm" onClick={toggleAll}>
							{allSelected ? t('mangaDetail.deselectAll') : t('mangaDetail.selectAll')}
						</Button>
						<span className="manga-chaps__selected">{t('mangaDetail.selectedCount', { count: selected.size })}</span>
						<Button size="sm" onClick={exitSelect}>
							{t('mangaDetail.done')}
						</Button>
					</div>
				) : (
					<div className="manga-chaps__tools">
						<div className="manga-chaps__query">
							<Input
								icon={<Search size={15} />}
								type="search"
								placeholder={t('mangaDetail.searchPlaceholder')}
								value={search}
								onChange={(e) => setSearch(e.currentTarget.value)}
							/>
							<div className="manga-chaps__order">
								<Select
									value={prefs.sort}
									title={t('mangaDetail.sortBy')}
									aria-label={t('mangaDetail.sortBy')}
									onChange={(e) => {
										const v = e.currentTarget.value as SortKey;
										setPrefs((p) => ({ ...p, sort: v }));
									}}
								>
									{SORT_OPTIONS.map((o) => (
										<option key={o.value} value={o.value}>
											{t(o.labelKey)}
										</option>
									))}
								</Select>
								<Button
									size="sm"
									title={prefs.asc ? t('mangaDetail.ascHint') : t('mangaDetail.descHint')}
									onClick={() => setPrefs((p) => ({ ...p, asc: !p.asc }))}
								>
									<ArrowDownUp size={14} />
									{prefs.asc ? t('mangaDetail.ascending') : t('mangaDetail.descending')}
								</Button>
							</div>
							{chapters.length > 0 && (
								<Button size="sm" onClick={() => setSelecting(true)}>
									{t('mangaDetail.select')}
								</Button>
							)}
						</div>
						<div className="manga-chaps__filters">
							{CHAP_FILTERS.map((f) => (
								<Chip
									key={f.value}
									triState={prefs.filters[f.value]}
									title={triTitle(t(f.labelKey), prefs.filters[f.value])}
									onClick={() =>
										setPrefs((p) => ({
											...p,
											filters: { ...p.filters, [f.value]: nextTri(p.filters[f.value]) }
										}))
									}
								>
									{t(f.labelKey)}
								</Chip>
							))}
						</div>
						{languages.length > 1 && (
							<Select
								value={prefs.language ?? ''}
								title={t('mangaDetail.language')}
								aria-label={t('mangaDetail.language')}
								onChange={(e) => {
									const v = e.currentTarget.value;
									setPrefs((p) => ({ ...p, language: v || null }));
								}}
							>
								<option value="">{t('mangaDetail.allLanguages')}</option>
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
								title={t('mangaDetail.scanlator')}
								aria-label={t('mangaDetail.scanlator')}
								onChange={(e) => {
									const v = e.currentTarget.value;
									setPrefs((p) => ({ ...p, scanlators: v ? [v] : [] }));
								}}
							>
								<option value="">{t('mangaDetail.allScanlators')}</option>
								{scanlators.map((s) => (
									<option key={s} value={s}>
										{s}
									</option>
								))}
							</Select>
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
						title={t('mangaDetail.chaptersLoadFailed')}
						hint={chaptersQuery.error?.message}
						action={
							<Button variant="primary" onClick={() => void chaptersQuery.refetch()}>
								{t('mangaDetail.retry')}
							</Button>
						}
					/>
				) : chapters.length === 0 ? (
					<EmptyState icon={BookOpen} title={t('mangaDetail.noChapters')} hint={t('mangaDetail.noChaptersHint')} />
				) : visible.length === 0 ? (
					<EmptyState icon={Search} title={t('mangaDetail.noMatch')} hint={t('mangaDetail.noMatchHint')} />
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
							downloadProgress={dlActivity.get(c.id)?.progress ?? undefined}
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
					<span className="manga-batch__count">{t('mangaDetail.selectedShort', { count: selected.size })}</span>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isRead: true }, (n) => t('mangaDetail.markedRead', { count: n }))}
					>
						{t('mangaDetail.markRead')}
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isRead: false }, (n) => t('mangaDetail.markedUnread', { count: n }))}
					>
						{t('mangaDetail.markUnread')}
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isBookmarked: true }, (n) => t('mangaDetail.bookmarked', { count: n }))}
					>
						{t('mangaDetail.addBookmark')}
					</Button>
					<Button
						size="sm"
						disabled={selected.size === 0 || batchBusy}
						onClick={() => batch({ isBookmarked: false }, (n) => t('mangaDetail.unbookmarked', { count: n }))}
					>
						{t('mangaDetail.removeBookmark')}
					</Button>
					{!isLocal && (
						<Button
							size="sm"
							variant="primary"
							disabled={selected.size === 0 || batchBusy}
							onClick={downloadSelected}
						>
							{t('mangaDetail.download')}
						</Button>
					)}
				</div>
			)}

			{migrateOpen && <MigrateModal manga={manga} onClose={() => setMigrateOpen(false)} />}

			{coverPickerOpen && <CoverPickerModal manga={manga} onClose={() => setCoverPickerOpen(false)} />}

			<Modal
				open={downloadRulesOpen}
				title={t('mangaDetail.downloadRulesTitle')}
				onClose={() => setDownloadRulesOpen(false)}
				footer={
					<>
						<Button onClick={() => setDownloadRulesOpen(false)}>{t('common.cancel')}</Button>
						<Button
							variant="primary"
							loading={enqueueDownloads.isPending}
							disabled={downloadRuleIds.length === 0 || enqueueDownloads.isPending}
							onClick={downloadByRule}
						>
							{t('mangaDetail.download')}
						</Button>
					</>
				}
			>
				<div className="manga-download-rules">
					<Select
						value={downloadRule}
						aria-label={t('mangaDetail.downloadRule')}
						onChange={(e) => {
							setDownloadRule(e.currentTarget.value as DownloadRule);
							setDownloadRuleQueued(null);
						}}
					>
						<option value="unread">{t('mangaDetail.downloadRuleUnread')}</option>
						<option value="latest">{t('mangaDetail.downloadRuleLatest')}</option>
						<option value="all">{t('mangaDetail.downloadRuleAll')}</option>
					</Select>
					{downloadRule === 'latest' && (
						<Input
							type="number"
							min={1}
							step={1}
							inputMode="numeric"
							aria-label={t('mangaDetail.downloadRuleLatestCount')}
							value={downloadLatest}
							onChange={(e) => {
								setDownloadLatest(e.currentTarget.value);
								setDownloadRuleQueued(null);
							}}
						/>
					)}
					<p className="muted">{t('mangaDetail.downloadRulesScope')}</p>
					<p>{t('mangaDetail.downloadRuleMatches', { count: downloadRuleIds.length })}</p>
					{downloadRuleQueued !== null && (
						<p role="status">{t('mangaDetail.queuedChapters', { count: downloadRuleQueued })}</p>
					)}
				</div>
			</Modal>

			<Modal
				open={removeOpen}
				title={t('mangaDetail.removeFromLibrary')}				onClose={() => setRemoveOpen(false)}
				footer={
					<>
						<Button onClick={() => setRemoveOpen(false)}>{t('common.cancel')}</Button>
						<Button variant="danger" loading={removeFromLibrary.isPending} onClick={confirmRemove}>
							{t('mangaDetail.remove')}
						</Button>
					</>
				}
			>
				<p>{t('mangaDetail.removeConfirm', { title: manga.title })}</p>
			</Modal>

			<Modal
				open={catOpen}
				title={t('mangaDetail.setCategories')}
				onClose={() => setCatOpen(false)}
				footer={
					allCats.length > 0 ? (
						<Button variant="primary" loading={setMangaCategories.isPending} onClick={saveCats}>
							{t('common.save')}
						</Button>
					) : undefined
				}
			>
				{allCats.length === 0 ? (
					<p className="muted">{t('mangaDetail.noCategories')}</p>
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
