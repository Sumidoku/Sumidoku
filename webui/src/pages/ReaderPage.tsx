import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, MouseEvent as RMouseEvent, PointerEvent as RPointerEvent } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { marked } from 'marked';
import {
	AlignJustify,
	ArrowDownUp,
	ArrowLeft,
	BookOpen,
	Bookmark,
	CheckCircle2,
	List,
	Lock,
	Settings,
	StretchVertical,
	TriangleAlert,
	X
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { api, pageUrl } from '../api/client';
import { useDocumentTitle } from '../components/DocumentTitle';
import { qk, useChapterPages, useManga, useMangaChapters } from '../api/hooks';
import type { ChapterWithDownload, Page } from '../api/types';
import { chapterLabel, formatDate, hasChapterNumber } from '../lib/format';
import { applyPrefs, DEFAULT_PREFS, loadPrefs, savePrefs } from '../lib/chapterPrefs';
import { Button, EmptyState, IconButton, SegmentedControl, Spinner } from '../components';
import { usePinchZoom } from './reader/usePinchZoom';
import { pageText } from './reader/pageText';
import { ReaderSettings } from './reader/ReaderSettings';
import { dirFromViewer, modeFromViewer, resolveDir, resolveMode, zoneMovesForward } from './reader/readerPrefs';
import type { Dir, Mode } from './reader/readerPrefs';
import { buildSpreads, spreadIndexOf, spreadLeadPage } from './reader/spreads';
import type { SpreadLayout } from './reader/spreads';
import { ReaderPager } from './reader/ReaderPager';
import { TextColumns } from './reader/TextColumns';
import { loadTypography, saveTypography, typographyStyle } from './reader/typography';
import type { ReaderTypography } from './reader/typography';
import './reader.css';

// continuous is the stored value for "scroll" in the old localStorage; renaming it would lose existing users' settings

const MODES: { key: Mode; icon: LucideIcon }[] = [
	{ key: 'paged', icon: BookOpen },
	{ key: 'continuous', icon: AlignJustify },
	{ key: 'webtoon', icon: StretchVertical }
];

function readMode(): Mode {
	try {
		const m = localStorage.getItem('sumidoku.reader.mode');
		if (m === 'paged' || m === 'continuous' || m === 'webtoon') return m;
		// The old settings page mistakenly stored "scroll" as vertical; treat it as continuous
		if (m === 'vertical') return 'continuous';
	} catch {
		// In private mode storage is unreadable, so fall back to the default
	}
	return 'paged';
}

function readDir(): Dir {
	try {
		if (localStorage.getItem('sumidoku.reader.dir') === 'rtl') return 'rtl';
	} catch {
		// Same as above
	}
	return 'ltr';
}

// Per-manga override of the metadata/default direction, keyed by manga id
function dirOverrideKey(mangaId: number): string {
	return `sumidoku.reader.dir.${mangaId}`;
}

function readDirOverride(mangaId: number | null): Dir | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(dirOverrideKey(mangaId));
		if (v === 'ltr' || v === 'rtl') return v;
	} catch {
		// Same as above
	}
	return null;
}

// Per-manga override of the metadata/default reading mode, keyed by manga id
function modeOverrideKey(mangaId: number): string {
	return `sumidoku.reader.mode.${mangaId}`;
}

function readModeOverride(mangaId: number | null): Mode | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(modeOverrideKey(mangaId));
		if (v === 'paged' || v === 'continuous' || v === 'webtoon') return v;
	} catch {
		// Same as above
	}
	return null;
}

// Page layout (single/double spread) has no manga-metadata counterpart, so the
// resolution is just per-manga override over the global default
function readLayout(): SpreadLayout {
	try {
		const v = localStorage.getItem('sumidoku.reader.layout');
		if (v === 'single' || v === 'double' || v === 'doubleNoCover') return v;
	} catch {
		// Same as above
	}
	return 'single';
}

function layoutOverrideKey(mangaId: number): string {
	return `sumidoku.reader.layout.${mangaId}`;
}

function readLayoutOverride(mangaId: number | null): SpreadLayout | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(layoutOverrideKey(mangaId));
		if (v === 'single' || v === 'double' || v === 'doubleNoCover') return v;
	} catch {
		// Same as above
	}
	return null;
}

/** The server 501s the Image variant outright, and the frontend has no way to render it either */
function isUnsupported(p: Page): boolean {
	return !!p.content && typeof p.content === 'object' && 'Image' in p.content;
}

function pageDimensions(p: Page | undefined | null): { width: number; height: number } | null {
	if (!p || typeof p.width !== 'number' || typeof p.height !== 'number') return null;
	if (!Number.isFinite(p.width) || !Number.isFinite(p.height) || p.width <= 0 || p.height <= 0) return null;
	return { width: p.width, height: p.height };
}

// Keep the old trust model: markdown output is not sanitized
function md(text: string): string {
	return marked.parse(text, { async: false }) as string;
}

function ReaderChapterItem({
	chapter: c,
	current,
	onClick
}: {
	chapter: ChapterWithDownload;
	current: boolean;
	onClick: () => void;
}) {
	const { t } = useTranslation();
	const uploaded = formatDate(c.date_uploaded);
	const scanlator = c.scanlators?.[0] ?? '';
	return (
		<div className={current ? 'reader__drawer-row--current' : undefined}>
			<button
				type="button"
				className={['chapter-row', c.isRead ? 'chapter-row--read' : ''].filter(Boolean).join(' ')}
				onClick={onClick}
			>
				<span className="chapter-row__num">{hasChapterNumber(c.chapter_number) ? c.chapter_number : ''}</span>
				<span className="chapter-row__main">
					<span className="chapter-row__title">{chapterLabel(c)}</span>
					{(uploaded || scanlator) && (
						<span className="chapter-row__meta">
							{uploaded && <span>{uploaded}</span>}
							{scanlator && <span>{scanlator}</span>}
						</span>
					)}
				</span>
				<span className="chapter-row__status">
					{c.locked && (
						<span className="chapter-row__lock" title={t('chapterRow.locked')}>
							<Lock size={14} />
						</span>
					)}
					{c.isBookmarked && (
						<span className="chapter-row__bm" title={t('chapterRow.bookmarked')}>
							<Bookmark size={14} fill="currentColor" />
						</span>
					)}
					{c.isDownloaded && (
						<span className="chapter-row__dl-ok" title={t('chapterRow.downloaded')}>
							<CheckCircle2 size={14} />
						</span>
					)}
					{!c.isRead && c.lastPageRead > 0 && (
						<span className="chapter-row__prog">P.{c.lastPageRead + 1}</span>
					)}
				</span>
			</button>
		</div>
	);
}

// Page-image downsampling target width = display container CSS width × DPR, bucketed up to 256 by the server; webtoon/paged fill the screen width, continuous caps at 780
function readImgWidth(mode: Mode): number {
	const css = mode === 'continuous' ? Math.min(window.innerWidth, 780) : window.innerWidth;
	return Math.max(1, Math.round(css * (window.devicePixelRatio || 1)));
}

export default function ReaderPage() {
	const { t } = useTranslation();
	const { chapterId: raw } = useParams();
	const [sp] = useSearchParams();
	const chapterId = Number(raw);
	const mangaRaw = sp.get('manga');
	const pageRaw = sp.get('page');
	const mangaId = mangaRaw !== null && Number.isFinite(Number(mangaRaw)) ? Number(mangaRaw) : null;
	const startPage = pageRaw !== null && Number.isFinite(Number(pageRaw)) ? Number(pageRaw) : null;

	if (!Number.isFinite(chapterId)) {
		return (
			<div className="reader reader--msg">
				<EmptyState icon={TriangleAlert} title={t('reader.invalidChapter')} />
			</div>
		);
	}
	// key pins on chapterId: switching chapters remounts the whole thing, resetting page number, report queue, and observer
	return <ReaderCore key={chapterId} chapterId={chapterId} mangaId={mangaId} startPage={startPage} />;
}

function ReaderCore({
	chapterId,
	mangaId,
	startPage
}: {
	chapterId: number;
	mangaId: number | null;
	startPage: number | null;
}) {
	const navigate = useNavigate();
	const qc = useQueryClient();
	const { t } = useTranslation();

	const pagesQuery = useChapterPages(chapterId);
	const pages = useMemo(() => pagesQuery.data ?? null, [pagesQuery.data]);

	const navEnabled = mangaId !== null;
	const mangaQuery = useManga(mangaId ?? 0, { enabled: navEnabled, silent: true });
	const chaptersQuery = useMangaChapters(mangaId ?? 0, { enabled: navEnabled, silent: true });
	// The raw list is kept for chapter titles/resume progress; the sorted/filtered list only drives prev/next-chapter navigation
	const rawChapters = useMemo(() => chaptersQuery.data ?? [], [chaptersQuery.data]);
	const prefs = useMemo(() => (mangaId !== null ? loadPrefs(mangaId) : DEFAULT_PREFS), [mangaId]);
	const [asc, setAsc] = useState(() => prefs.asc);
	// Trashed chapters are unreadable (the API refuses them): navigation and the drawer skip them; the raw
	// list keeps them so the current chapter's title/progress still resolve
	const chapters = useMemo(() => applyPrefs(rawChapters, { ...prefs, asc }).filter((c) => !c.trashed), [rawChapters, prefs, asc]);
	const navChapters = useMemo(() => (asc ? [...chapters].reverse() : chapters), [chapters, asc]);

	const [globalMode] = useState<Mode>(readMode);
	const [modeOverride, setModeOverride] = useState<Mode | null>(() => readModeOverride(mangaId));
	const [globalDir] = useState<Dir>(readDir);
	const [dirOverride, setDirOverride] = useState<Dir | null>(() => readDirOverride(mangaId));
	const [globalLayout] = useState<SpreadLayout>(readLayout);
	const [layoutOverride, setLayoutOverride] = useState<SpreadLayout | null>(() => readLayoutOverride(mangaId));
	const layout = layoutOverride ?? globalLayout;
	// Per-manga override > manga metadata (viewer) > global default, for mode and direction alike
	const metaMode = modeFromViewer(mangaQuery.data?.viewer);
	const metaDir = dirFromViewer(mangaQuery.data?.viewer);
	const mode = resolveMode(modeOverride, metaMode, globalMode);
	const dir = resolveDir(dirOverride, metaDir, globalDir);
	const [overlay, setOverlay] = useState(true);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [chaptersOpen, setChaptersOpen] = useState(false);
	const drawerBodyRef = useRef<HTMLDivElement>(null);
	const [imgWidth, setImgWidth] = useState(() => readImgWidth(mode));
	const [typography, setTypography] = useState<ReaderTypography>(loadTypography);
	const textStyle = typographyStyle(typography);

	function updateTypography(patch: Partial<ReaderTypography>) {
		const next = { ...typography, ...patch };
		setTypography(next);
		saveTypography(next);
	}

	const idxRef = useRef(0);
	const [idx, setIdxState] = useState(0);
	const furthestRef = useRef(0);
	// In continuous/webtoon mode, the page at the viewport top, used for the page display; furthest is only for progress reporting
	const curRef = useRef(0);
	const [cur, setCurState] = useState(0);
	const initedRef = useRef(false);
	const contRef = useRef<HTMLDivElement>(null);
	const contentResizeRef = useRef<() => void>(() => {});

	// In paged mode a text page is split into viewport-wide columns, col being the current one; textLayout is the measured column width/gap/count
	const colRef = useRef(0);
	const [col, setColState] = useState(0);
	const [textLayout, setTextLayout] = useState({ w: 0, gap: 0, cols: 1 });
	const textLayoutRef = useRef(textLayout);
	textLayoutRef.current = textLayout;
	const textColsRef = useRef<HTMLDivElement>(null);

	const {
		targetRef: zoomRef,
		zoomed,
		touchAction,
		onPointerDown: pinchPointerDown,
		onPointerMove: pinchPointerMove,
		onPointerUp: pinchPointerUp,
		onPointerCancel: pinchPointerCancel,
		consumeClick,
		reset: resetPinch
	} = usePinchZoom({ zoomedTouchAction: mode === 'paged' ? 'none' : 'pan-y' });
	// Layout changes leave stale pan offsets behind
	useLayoutEffect(() => {
		resetPinch();
	}, [mode, idx, col, dir, layout, imgWidth, resetPinch]);

	const total = pages?.length ?? 0;
	const curPage = pages?.[idx] ?? null;
	const curText = pageText(curPage);
	// Spreads only exist in paged mode; slots carry page indexes so progress and the
	// slider stay page-based regardless of the layout
	const spreads = useMemo(() => {
		if (mode !== 'paged' || !pages) return null;
		return buildSpreads(
			pages.length,
			(i) => {
				const d = pageDimensions(pages[i]);
				return d !== null && d.width > d.height;
			},
			layout
		);
	}, [mode, pages, layout]);
	const curSpread = spreads ? spreads[spreadIndexOf(spreads, idx)] : null;
	// A text chapter is a single page on the server, so its pagination unit is the column, not the page
	const textOnly = mode === 'paged' && total === 1 && pageText(pages?.[0]) !== null;
	// Title/resume use the raw list, so the subtitle and progress survive the current chapter being filtered out
	const curChapter = rawChapters.find((c) => c.id === chapterId) ?? null;
	const pos = navChapters.findIndex((c) => c.id === chapterId);
	const prevCh = pos >= 0 && pos < navChapters.length - 1 ? navChapters[pos + 1] : null;
	const nextCh = pos > 0 ? navChapters[pos - 1] : null;
	const shown = mode === 'paged' ? idx : cur;
	// The pagination unit of a text-only chapter is the column; everything else's is the page
	const navTotal = textOnly ? textLayout.cols : total;
	const navPos = textOnly ? col : shown;
	const mangaTitle = mangaQuery.data?.title ?? '';

	const MODE_LABEL: Record<Mode, string> = {
		paged: t('reader.mode.paged'),
		continuous: t('reader.mode.continuous'),
		webtoon: t('reader.mode.webtoon')
	};

	function setIdx(i: number) {
		idxRef.current = i;
		setIdxState(i);
	}

	function setFurthest(i: number) {
		furthestRef.current = i;
	}

	function setCur(i: number) {
		curRef.current = i;
		setCurState(i);
	}

	function setCol(c: number) {
		colRef.current = c;
		setColState(c);
	}

	// After a window resize, refetch variants at the new width; debounced so dragging the window doesn't refetch images repeatedly
	useEffect(() => {
		setImgWidth(readImgWidth(mode));
		let timer: ReturnType<typeof setTimeout> | undefined;
		const onResize = () => {
			if (timer !== undefined) clearTimeout(timer);
			timer = setTimeout(() => setImgWidth(readImgWidth(mode)), 200);
		};
		window.addEventListener('resize', onResize);
		return () => {
			if (timer !== undefined) clearTimeout(timer);
			window.removeEventListener('resize', onResize);
		};
	}, [mode]);

	const reportRef = useRef<{
		timer: ReturnType<typeof setTimeout> | undefined;
		pending: { page: number; atEnd: boolean } | null;
		readReported: boolean;
	}>({ timer: undefined, pending: null, readReported: false });

	// Progress reports debounce 300ms and fail silently without interrupting reading; reaching the chapter end also marks it read, once per chapter
	function flushProgress() {
		const r = reportRef.current;
		if (r.timer !== undefined) {
			clearTimeout(r.timer);
			r.timer = undefined;
		}
		if (r.pending === null) return;
		const { page: i, atEnd } = r.pending;
		r.pending = null;
		const body: { lastPageRead: number; isRead?: boolean } = { lastPageRead: i };
		const markRead = total > 0 && atEnd && !r.readReported;
		if (markRead) body.isRead = true;
		api(`/chapter/${chapterId}`, { method: 'PATCH', body, silent: true })
			.then(() => {
				if (markRead) r.readReported = true;
				if (mangaId !== null) {
					qc.setQueryData<ChapterWithDownload[]>(qk.chapters(mangaId), (old) =>
						old?.map((c) =>
							c.id === chapterId ? { ...c, lastPageRead: i, isRead: markRead ? true : c.isRead } : c
						)
					);
				}
			})
			.catch(() => {});
	}

	function scheduleProgress(i: number, atEnd: boolean) {
		const r = reportRef.current;
		r.pending = { page: i, atEnd };
		if (r.timer !== undefined) clearTimeout(r.timer);
		r.timer = setTimeout(flushProgress, 300);
	}

	// The debounce queue may still hold unreported progress on exit; flush it before unmounting
	const flushRef = useRef(flushProgress);
	flushRef.current = flushProgress;
	useEffect(() => () => flushRef.current(), []);

	function prefetch(center: number) {
		if (!pages) return;
		for (let d = -3; d <= 3; d++) {
			if (d === 0) continue;
			const p = pages[center + d];
			if (!p || pageText(p) !== null) continue;
			const im = new Image();
			im.src = pageUrl(chapterId, p.index, imgWidth);
		}
	}

	function gotoPage(i: number) {
		const clamped = Math.min(Math.max(i, 0), Math.max(total - 1, 0));
		if (clamped === idxRef.current && pages) return;
		setIdx(clamped);
		setCol(0);
		if (clamped > furthestRef.current) setFurthest(clamped);
		scheduleProgress(clamped, clamped >= total - 1);
		prefetch(clamped);
	}

	function navCh(c: ChapterWithDownload | null) {
		if (!c) return;
		navigate(`/reader/${c.id}${mangaId !== null ? `?manga=${mangaId}` : ''}`, { replace: true });
	}

	// Hitting a boundary switches chapters outright: past the last page = next chapter, before the first page = previous chapter
	// Text pages turn column by column first; only when the columns run out does the page/chapter boundary apply
	// Double-page layouts turn spread by spread, landing on each spread's leading page in reading order
	function turn(forward: boolean) {
		const textPage = curText !== null;
		const cols = textLayoutRef.current.cols;
		if (forward) {
			if (textPage && colRef.current < cols - 1) {
				const nextCol = colRef.current + 1;
				setCol(nextCol);
				scheduleProgress(idxRef.current, idxRef.current >= total - 1 && nextCol >= cols - 1);
				return;
			}
			if (spreads) {
				const si = spreadIndexOf(spreads, idxRef.current);
				if (si >= spreads.length - 1) navCh(nextCh);
				else gotoPage(spreadLeadPage(spreads[si + 1]));
				return;
			}
			if (idxRef.current >= total - 1) navCh(nextCh);
			else gotoPage(idxRef.current + 1);
		} else {
			if (textPage && colRef.current > 0) {
				setCol(colRef.current - 1);
				scheduleProgress(idxRef.current, false);
				return;
			}
			if (spreads) {
				const si = spreadIndexOf(spreads, idxRef.current);
				if (si <= 0) navCh(prevCh);
				else gotoPage(spreadLeadPage(spreads[si - 1]));
				return;
			}
			if (idxRef.current <= 0) navCh(prevCh);
			else gotoPage(idxRef.current - 1);
		}
	}

	function onZone(which: 'left' | 'right') {
		turn(zoneMovesForward(dir, which));
	}

	function goBack() {
		// If there is history to go back to, pop it (e.g. back to manga detail).
		// Otherwise this is a deep link; fall back to the manga detail page or library.
		if (window.history.state?.idx > 0) navigate(-1);
		else if (mangaId !== null) navigate(`/manga/${mangaId}`);
		else navigate('/library');
	}

	const keyRef = useRef<(e: KeyboardEvent) => void>(() => {});
	keyRef.current = (e) => {
		if (e.key === 'Escape') {
			if (chaptersOpen) {
				setChaptersOpen(false);
				return;
			}
			if (settingsOpen) {
				setSettingsOpen(false);
				return;
			}
			goBack();
			return;
		}
		if (chaptersOpen) return;
		if (mode !== 'paged') return;
		if (e.key === 'ArrowLeft') onZone('left');
		else if (e.key === 'ArrowRight') onZone('right');
	};
	useEffect(() => {
		const h = (e: KeyboardEvent) => keyRef.current(e);
		window.addEventListener('keydown', h);
		return () => window.removeEventListener('keydown', h);
	}, []);

	useEffect(() => {
		if (!chaptersOpen) return;
		const current = drawerBodyRef.current?.querySelector('.reader__drawer-row--current');
		if (current) {
			const id = requestAnimationFrame(() => current.scrollIntoView({ block: 'center', behavior: 'instant' }));
			return () => cancelAnimationFrame(id);
		}
	}, [chaptersOpen, asc]);

	useDocumentTitle(mangaTitle ? t('reader.documentTitle', { title: mangaTitle }) : null);

	function setMode(m: Mode) {
		if (m === mode) return;
		try {
			if (mangaId !== null) {
				// Switching back to the metadata mode drops the override so future metadata edits apply
				if (m === metaMode) localStorage.removeItem(modeOverrideKey(mangaId));
				else localStorage.setItem(modeOverrideKey(mangaId), m);
			} else {
				// Without a manga in scope there is no metadata to respect: write the global default
				localStorage.setItem('sumidoku.reader.mode', m);
			}
		} catch {
			// Private mode blocks persistence but the mode still switches
		}
		setModeOverride(mangaId !== null && m !== metaMode ? m : null);
	}

	function setDir(d: Dir) {
		try {
			if (mangaId !== null) {
				// Toggling back to the metadata direction drops the override so future metadata edits apply
				if (d === metaDir) localStorage.removeItem(dirOverrideKey(mangaId));
				else localStorage.setItem(dirOverrideKey(mangaId), d);
			} else {
				// Without a manga in scope there is no metadata to respect: write the global default
				localStorage.setItem('sumidoku.reader.dir', d);
			}
		} catch {
			// Private mode blocks persistence but the direction still switches
		}
		setDirOverride(mangaId !== null && d !== metaDir ? d : null);
	}

	function setLayout(l: SpreadLayout) {
		if (l === layout) return;
		try {
			if (mangaId !== null) localStorage.setItem(layoutOverrideKey(mangaId), l);
			else localStorage.setItem('sumidoku.reader.layout', l);
		} catch {
			// Private mode blocks persistence but the layout still switches
		}
		setLayoutOverride(mangaId !== null ? l : null);
	}

	function toggleAsc() {
		const next = !asc;
		setAsc(next);
		if (mangaId !== null) savePrefs(mangaId, { ...prefs, asc: next });
	}

	function scrollToPage(i: number) {
		contRef.current
			?.querySelector(`[data-page="${i}"]`)
			?.scrollIntoView({ block: 'start', behavior: 'instant' });
	}

	// Start page: ?page > chapter reading progress > 0; don't move before chapter navigation data and the
	// manga's viewer metadata are in place, to avoid landing on the wrong progress or in the wrong mode
	const navReady = !navEnabled || (chaptersQuery.isFetched && mangaQuery.isFetched);
	useLayoutEffect(() => {
		if (!pages || !navReady || initedRef.current) return;
		initedRef.current = true;
		const resume = rawChapters.find((c) => c.id === chapterId)?.lastPageRead ?? 0;
		const initial = Math.min(
			Math.max(startPage ?? (resume > 0 ? resume : 0), 0),
			Math.max(pages.length - 1, 0)
		);
		setIdx(initial);
		setFurthest(initial);
		setCol(0);
		setCur(initial);
		prefetch(initial);
		if (mode !== 'paged') scrollToPage(initial);
	});

	// After switching to continuous/webtoon, position back to the current page
	const prevModeRef = useRef(mode);
	useLayoutEffect(() => {
		if (prevModeRef.current === mode) return;
		prevModeRef.current = mode;
		if (mode === 'paged') {
			// Returning to paged aligns to the continuous mode's current page, not the idx left behind when switching away
			setIdx(curRef.current);
			setCol(0);
		} else {
			// Entering continuous/webtoon returns the page number to the paged mode's current page, not the historical furthest page
			setCur(idxRef.current);
			if (initedRef.current) scrollToPage(idxRef.current);
		}
	}, [mode]);

	// In paged mode, split text pages into viewport-wide equal columns; the measured column count drives pagination and section jumps
	useLayoutEffect(() => {
		if (mode !== 'paged') return;
		const el = textColsRef.current;
		if (!el) return;
		const recompute = () => {
			const w = el.clientWidth;
			if (w <= 0) return;
			el.style.columnWidth = `${w}px`;
			const gap = parseFloat(getComputedStyle(el).columnGap) || 0;
			const cols = Math.max(1, Math.round((el.scrollWidth + gap) / (w + gap)));
			setTextLayout((prev) =>
				prev.w === w && prev.gap === gap && prev.cols === cols ? prev : { w, gap, cols }
			);
			if (colRef.current > cols - 1) setCol(cols - 1);
		};
		recompute();
		const ro = new ResizeObserver(recompute);
		ro.observe(el);
		return () => ro.disconnect();
	}, [mode, idx, pages, typography]);

	// Continuous/webtoon progress = the highest page number that has entered the viewport
	useEffect(() => {
		if (mode === 'paged' || !pages || !navReady) return;
		const root = contRef.current;
		if (!root) return;
		const els = root.querySelectorAll('[data-page]');
		const ob = new IntersectionObserver((es) => {
			for (const e of es) {
				if (!e.isIntersecting) continue;
				const i = Number((e.target as HTMLElement).dataset.page);
				if (i > furthestRef.current) {
					setFurthest(i);
					scheduleProgress(i, i >= pages.length - 1);
				}
			}
		});
		els.forEach((el) => ob.observe(el));
		return () => ob.disconnect();
	}, [mode, pages, navReady]);

	// Continuous/webtoon page number = the page covering the viewport top, updated live while scrolling
	useEffect(() => {
		if (mode === 'paged' || !pages || !navReady) return;
		const root = contRef.current;
		if (!root) return;
		let raf = 0;
		const update = () => {
			raf = 0;
			const els = root.querySelectorAll<HTMLElement>('[data-page]');
			let best: { i: number; score: number } | null = null;
			for (const el of els) {
				const r = el.getBoundingClientRect();
				const score = r.top <= 0 && r.bottom > 0 ? 0 : Math.abs(r.top);
				if (best === null || score < best.score) best = { i: Number(el.dataset.page), score };
			}
			if (best !== null && best.i !== curRef.current) setCur(best.i);
		};
		const onScroll = () => {
			if (raf === 0) raf = requestAnimationFrame(update);
		};
		contentResizeRef.current = onScroll;
		update();
		root.addEventListener('scroll', onScroll, { passive: true });
		// Lazy content can change a page box without a scroll event, so recompute the visible page as they resize
		const ro = new ResizeObserver(onScroll);
		ro.observe(root);
		root.querySelectorAll('[data-page]').forEach((el) => ro.observe(el));
		return () => {
			contentResizeRef.current = () => {};
			root.removeEventListener('scroll', onScroll);
			ro.disconnect();
			if (raf) cancelAnimationFrame(raf);
		};
	}, [mode, pages, navReady]);

	// A light tap in the middle summons the toolbar; swipes don't trigger it
	const downRef = useRef({ x: 0, y: 0 });
	function onPagedClick(which: 'left' | 'right' | 'mid') {
		if (consumeClick()) return;
		if (zoomed || which === 'mid') {
			setOverlay((v) => !v);
			return;
		}
		onZone(which);
	}
	function onPagedContainerClick(e: RMouseEvent<HTMLDivElement>) {
		if (e.target !== e.currentTarget) return;
		const rect = e.currentTarget.getBoundingClientRect();
		const ratio = (e.clientX - rect.left) / rect.width;
		onPagedClick(ratio < 0.28 ? 'left' : ratio > 0.72 ? 'right' : 'mid');
	}
	function onContinuousPointerDown(e: RPointerEvent<HTMLDivElement>) {
		downRef.current = { x: e.clientX, y: e.clientY };
		pinchPointerDown(e);
	}
	function onContinuousClick(e: RMouseEvent<HTMLDivElement>) {
		if (consumeClick()) return;
		if (Math.hypot(e.clientX - downRef.current.x, e.clientY - downRef.current.y) > 10) return;
		if (zoomed) {
			setOverlay((v) => !v);
			return;
		}
		const third = window.innerWidth / 3;
		if (e.clientX > third && e.clientX < third * 2) setOverlay((v) => !v);
	}

	function onSlider(e: ChangeEvent<HTMLInputElement>) {
		const i = Number(e.currentTarget.value) - 1;
		if (mode === 'paged') {
			if (textOnly) {
				const cols = textLayoutRef.current.cols;
				const c = Math.min(Math.max(i, 0), Math.max(cols - 1, 0));
				setCol(c);
				scheduleProgress(idxRef.current, c >= cols - 1);
			} else {
				gotoPage(i);
			}
			return;
		}
		scrollToPage(i);
		setIdx(i);
		setCur(i);
		if (i > furthestRef.current) {
			setFurthest(i);
			scheduleProgress(i, i >= total - 1);
		}
	}

	const barCls = (base: string) => (overlay ? base : `${base} reader__bar--hidden`);

	if (pagesQuery.isPending) {
		return (
			<div className="reader reader--msg">
				<Spinner />
			</div>
		);
	}
	if (pagesQuery.isError) {
		return (
			<div className="reader reader--msg">
				<EmptyState
					icon={TriangleAlert}
					title={t('reader.loadFailed')}
					hint={pagesQuery.error?.message}
					action={
						<Button variant="primary" onClick={() => void pagesQuery.refetch()}>
							{t('reader.retry')}
						</Button>
					}
				/>
			</div>
		);
	}
	if (!pages || pages.length === 0) {
		return (
			<div className="reader reader--msg">
				<EmptyState
					icon={BookOpen}
					title={t('reader.noPages')}
					action={<Button onClick={goBack}>{t('reader.back')}</Button>}
				/>
			</div>
		);
	}

	return (
		<div className="reader">
			{mode === 'paged' ? (
				<div
					className="reader__paged"
					style={{ touchAction }}
					onClick={onPagedContainerClick}
					onPointerDown={pinchPointerDown}
					onPointerMove={pinchPointerMove}
					onPointerUp={pinchPointerUp}
					onPointerCancel={pinchPointerCancel}
				>
					<div
						className="reader__zoom"
						ref={zoomRef}
						style={{ transformOrigin: '0 0', willChange: zoomed ? 'transform' : undefined }}
					>
						{curPage &&
							(curText !== null ? (
								<TextColumns
									dir={dir}
									html={md(curText)}
									textStyle={textStyle}
									layout={textLayout}
									col={col}
									innerRef={textColsRef}
								/>
							) : (
								// Slots are in reading order; the flex row's direction puts the first
								// slot on the leading side (right under rtl)
								<div
									className="reader__spread"
									style={{ direction: dir }}
									data-slots={(curSpread ?? [idx]).length}
								>
									{(curSpread ?? [idx]).map((slot, si) => {
										if (slot === null || !pages?.[slot]) {
											return <div key={`blank-${si}`} className="reader__blank" />;
										}
										const p = pages[slot];
										const slotText = pageText(p);
										const dims = pageDimensions(p);
										if (slotText !== null) {
											return (
												<div
													key={p.index}
													className="reader-text reader__spread-text"
													style={textStyle}
													dangerouslySetInnerHTML={{ __html: md(slotText) }}
												/>
											);
										}
										if (isUnsupported(p)) {
											return (
												<div key={p.index} className="reader__unsupported">
													{t('reader.unsupportedPage')}
												</div>
											);
										}
										return (
											<img
												key={p.index}
												className="reader__img"
												src={pageUrl(chapterId, p.index, imgWidth)}
												alt={t('reader.pageAlt', { page: slot + 1 })}
												width={dims?.width}
												height={dims?.height}
												draggable={false}
											/>
										);
									})}
								</div>
							))}
					</div>
					<button
						type="button"
						className="reader__zone reader__zone--left"
						aria-label={t(zoneMovesForward(dir, 'left') ? 'reader.nextPage' : 'reader.prevPage')}
						onClick={(e) => {
							e.stopPropagation();
							onPagedClick('left');
						}}
					/>
					<button
						type="button"
						className="reader__zone reader__zone--right"
						aria-label={t(zoneMovesForward(dir, 'right') ? 'reader.nextPage' : 'reader.prevPage')}
						onClick={(e) => {
							e.stopPropagation();
							onPagedClick('right');
						}}
					/>
					<button
						type="button"
						className="reader__zone reader__zone--mid"
						aria-label={t('reader.toggleToolbar')}
						onClick={(e) => {
							e.stopPropagation();
							onPagedClick('mid');
						}}
					/>
				</div>
			) : (
				<div
					className="reader__continuous"
					style={{ touchAction }}
					onClick={onContinuousClick}
					onPointerDown={onContinuousPointerDown}
					onPointerMove={pinchPointerMove}
					onPointerUp={pinchPointerUp}
					onPointerCancel={pinchPointerCancel}
				>
					<div
						className="reader__zoom"
						ref={zoomRef}
						style={{ transformOrigin: '0 0', willChange: zoomed ? 'transform' : undefined }}
					>
						<div
							className={mode === 'webtoon' ? 'reader__cont reader__cont--webtoon' : 'reader__cont'}
							ref={contRef}
						>
							{pages.map((p) => {
								const text = pageText(p);
								const unsupported = isUnsupported(p);
								const dimensions = text === null && !unsupported ? pageDimensions(p) : null;
								return (
									<div
										className="reader__pg"
										data-page={p.index}
										key={p.index}
										style={dimensions ? { aspectRatio: `${dimensions.width} / ${dimensions.height}` } : undefined}
									>
										{text !== null ? (
											<div className="reader-text" style={textStyle} dangerouslySetInnerHTML={{ __html: md(text) }} />
										) : unsupported ? (
											<div className="reader__unsupported">{t('reader.unsupportedPage')}</div>
										) : (
											<img
												src={pageUrl(chapterId, p.index, imgWidth)}
												alt={t('reader.pageAlt', { page: p.index + 1 })}
												width={dimensions?.width}
												height={dimensions?.height}
												loading="lazy"
												onLoad={() => contentResizeRef.current()}
												draggable={false}
											/>
										)}
									</div>
								);
							})}
						</div>
					</div>
				</div>
			)}

			<div className={barCls('reader__bar reader__bar--top')}>
				<IconButton label={t('reader.back')} onClick={goBack}>
					<ArrowLeft size={18} />
				</IconButton>
				<div className="reader__titles">
					<div className="reader__title">{mangaTitle || t('reader.reading')}</div>
					{curChapter && <div className="reader__subtitle">{chapterLabel(curChapter)}</div>}
				</div>
				<SegmentedControl
					options={MODES.map((m) => ({
						value: m.key,
						label: (
							<span className="reader__mode" title={MODE_LABEL[m.key]}>
								<m.icon size={15} />
								<span className="reader__mode-label">{MODE_LABEL[m.key]}</span>
							</span>
						)
					}))}
					value={mode}
					onChange={setMode}
				/>
				{navEnabled && (
					<IconButton
						label={t('reader.chapters')}
						className={chaptersOpen ? 'reader__icon--on' : undefined}
						onClick={() => setChaptersOpen((v) => !v)}
						disabled={!chaptersQuery.isFetched || navChapters.length === 0}
					>
						<List size={17} />
					</IconButton>
				)}
				<IconButton
					label={t('reader.settings')}
					className={settingsOpen ? 'reader__icon--on' : undefined}
					onClick={() => setSettingsOpen((v) => !v)}
				>
					<Settings size={17} />
				</IconButton>
			</div>

			{overlay && settingsOpen && (
				<ReaderSettings
					mode={mode}
					dir={dir}
					onDirChange={setDir}
					layout={layout}
					onLayoutChange={setLayout}
					currentPage={curPage}
					typography={typography}
					onTypographyChange={updateTypography}
				/>
			)}

			{chaptersOpen && (
				<>
					<div
						className="reader__drawer-mask"
						onClick={() => setChaptersOpen(false)}
						aria-hidden="true"
					/>
					<div className="reader__drawer">
					<div className="reader__drawer-head">
						<span className="reader__drawer-title">{t('reader.chapters')}</span>
						<div className="reader__drawer-actions">
							<IconButton
								label={asc ? t('reader.ascending') : t('reader.descending')}
								onClick={toggleAsc}
							>
								<ArrowDownUp size={17} />
							</IconButton>
							<IconButton label={t('common.cancel')} onClick={() => setChaptersOpen(false)}>
								<X size={17} />
							</IconButton>
						</div>
					</div>
						<div className="reader__drawer-body" ref={drawerBodyRef}>
							{chaptersQuery.isPending ? (
								<div className="reader__drawer-msg">
									<Spinner />
								</div>
							) : chaptersQuery.isError ? (
								<div className="reader__drawer-msg">{chaptersQuery.error?.message}</div>
							) : navChapters.length === 0 ? (
								<div className="reader__drawer-msg">{t('reader.noChapters')}</div>
							) : (
								navChapters.map((c) => (
									<ReaderChapterItem
										key={c.id}
										chapter={c}
										current={c.id === chapterId}
										onClick={() => {
											setChaptersOpen(false);
											if (c.id !== chapterId) {
												navigate(`/reader/${c.id}?manga=${mangaId}`, { replace: true });
											}
										}}
									/>
								))
							)}
						</div>
					</div>
				</>
			)}

			<ReaderPager
				dir={dir}
				hidden={!overlay}
				navPos={navPos}
				navTotal={navTotal}
				prevCh={prevCh}
				nextCh={nextCh}
				onSlider={onSlider}
				onNavChapter={navCh}
			/>
		</div>
	);
}
