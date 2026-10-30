import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, CSSProperties, MouseEvent as RMouseEvent, PointerEvent as RPointerEvent } from 'react';
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
	Maximize,
	Minimize,
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
import { useResolvedTheme } from '../lib/theme';
import { Button, EmptyState, IconButton, SegmentedControl, Spinner } from '../components';
import { usePinchZoom } from './reader/usePinchZoom';
import { pageText } from './reader/pageText';
import { ReaderSettings } from './reader/ReaderSettings';
import { TransitionPage } from './reader/TransitionPage';
import {
	dirFromViewer,
	modeFromViewer,
	nextScale,
	parseBackground,
	parseScale,
	parseTapZones,
	resolveDir,
	resolveMode,
	zoneAction,
	zoneAt,
	zoneMovesForward
} from './reader/readerPrefs';
import type { Dir, Mode, PageScale, ReaderBackground, TapZoneLayout } from './reader/readerPrefs';
import { buildSpreads, measureWhenKnown, recordMeasured, slotAspectRatio, spreadIndexOf, spreadLeadPage, trackOffsetPercent } from './reader/spreads';
import type { PageDims, SpreadLayout } from './reader/spreads';
import { isSplittable, splitEndReached, splitSides, turnSplit } from './reader/splitWide';
import { readerKeyAction } from './reader/hotkeys';
import { WheelTurnController } from './reader/wheelTurn';
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

// Tap-zone layout and canvas background follow the page layout's persistence
// shape: a global default plus a per-manga override, no metadata layer
function readTapZones(): TapZoneLayout {
	try {
		const v = parseTapZones(localStorage.getItem('sumidoku.reader.tapzones'));
		if (v !== null) return v;
	} catch {
		// Same as above
	}
	return 'lr';
}

function tapZonesOverrideKey(mangaId: number): string {
	return `sumidoku.reader.tapzones.${mangaId}`;
}

function readTapZonesOverride(mangaId: number | null): TapZoneLayout | null {
	if (mangaId === null) return null;
	try {
		return parseTapZones(localStorage.getItem(tapZonesOverrideKey(mangaId)));
	} catch {
		// Same as above
	}
	return null;
}

function readBackground(): ReaderBackground {
	try {
		const v = parseBackground(localStorage.getItem('sumidoku.reader.bg'));
		if (v !== null) return v;
	} catch {
		// Same as above
	}
	return 'dark';
}

function backgroundOverrideKey(mangaId: number): string {
	return `sumidoku.reader.bg.${mangaId}`;
}

function readBackgroundOverride(mangaId: number | null): ReaderBackground | null {
	if (mangaId === null) return null;
	try {
		return parseBackground(localStorage.getItem(backgroundOverrideKey(mangaId)));
	} catch {
		// Same as above
	}
	return null;
}

// Page scale and small-page stretching share the same persistence shape: a
// global default plus a per-manga override, no metadata layer
function readScale(): PageScale {
	try {
		const v = parseScale(localStorage.getItem('sumidoku.reader.scale'));
		if (v !== null) return v;
	} catch {
		// Same as above
	}
	return 'screen';
}

function scaleOverrideKey(mangaId: number): string {
	return `sumidoku.reader.scale.${mangaId}`;
}

function readScaleOverride(mangaId: number | null): PageScale | null {
	if (mangaId === null) return null;
	try {
		return parseScale(localStorage.getItem(scaleOverrideKey(mangaId)));
	} catch {
		// Same as above
	}
	return null;
}

// The stretch flag stores only "on"; anything else (including absence) is off
function readStretch(): boolean {
	try {
		return localStorage.getItem('sumidoku.reader.stretch') === '1';
	} catch {
		return false;
	}
}

function stretchOverrideKey(mangaId: number): string {
	return `sumidoku.reader.stretch.${mangaId}`;
}

function readStretchOverride(mangaId: number | null): boolean | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(stretchOverrideKey(mangaId));
		if (v === '1') return true;
		if (v === '0') return false;
	} catch {
		// Same as above
	}
	return null;
}

// Same storage convention as stretch: only "1" is on
function readSplit(): boolean {
	try {
		return localStorage.getItem('sumidoku.reader.split') === '1';
	} catch {
		return false;
	}
}

function splitOverrideKey(mangaId: number): string {
	return `sumidoku.reader.split.${mangaId}`;
}

function readSplitOverride(mangaId: number | null): boolean | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(splitOverrideKey(mangaId));
		if (v === '1') return true;
		if (v === '0') return false;
	} catch {
		// Same as above
	}
	return null;
}

// The turn animation defaults to on, so only "0" is stored; overrides keep "1"/"0"
function readAnim(): boolean {
	try {
		return localStorage.getItem('sumidoku.reader.anim') !== '0';
	} catch {
		return true;
	}
}

function animOverrideKey(mangaId: number): string {
	return `sumidoku.reader.anim.${mangaId}`;
}

function readAnimOverride(mangaId: number | null): boolean | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(animOverrideKey(mangaId));
		if (v === '1') return true;
		if (v === '0') return false;
	} catch {
		// Same as above
	}
	return null;
}

// The chapter transition page defaults to on, so only "0" disables it
function readTransition(): boolean {
	try {
		return localStorage.getItem('sumidoku.reader.transition') !== '0';
	} catch {
		return true;
	}
}

function transitionOverrideKey(mangaId: number): string {
	return `sumidoku.reader.transition.${mangaId}`;
}

function readTransitionOverride(mangaId: number | null): boolean | null {
	if (mangaId === null) return null;
	try {
		const v = localStorage.getItem(transitionOverrideKey(mangaId));
		if (v === '1') return true;
		if (v === '0') return false;
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
	const [globalTapZones] = useState<TapZoneLayout>(readTapZones);
	const [tapZonesOverride, setTapZonesOverride] = useState<TapZoneLayout | null>(() => readTapZonesOverride(mangaId));
	const tapZones = tapZonesOverride ?? globalTapZones;
	const [globalBg] = useState<ReaderBackground>(readBackground);
	const [bgOverride, setBgOverride] = useState<ReaderBackground | null>(() => readBackgroundOverride(mangaId));
	const bg = bgOverride ?? globalBg;
	// 'auto' follows the app theme: a light app theme reads on a white canvas
	const resolvedTheme = useResolvedTheme();
	const effectiveBg = bg === 'auto' ? (resolvedTheme === 'light' ? 'light' : 'dark') : bg;
	const [globalScale] = useState<PageScale>(readScale);
	const [scaleOverride, setScaleOverride] = useState<PageScale | null>(() => readScaleOverride(mangaId));
	const scale = scaleOverride ?? globalScale;
	const [globalStretch] = useState<boolean>(readStretch);
	const [stretchOverride, setStretchOverride] = useState<boolean | null>(() => readStretchOverride(mangaId));
	const stretch = stretchOverride ?? globalStretch;
	const [globalSplit] = useState<boolean>(readSplit);
	const [splitOverride, setSplitOverride] = useState<boolean | null>(() => readSplitOverride(mangaId));
	const split = splitOverride ?? globalSplit;
	const [globalAnim] = useState<boolean>(readAnim);
	const [animOverride, setAnimOverride] = useState<boolean | null>(() => readAnimOverride(mangaId));
	const anim = animOverride ?? globalAnim;
	const [globalTransition] = useState<boolean>(readTransition);
	const [transitionOverride, setTransitionOverride] = useState<boolean | null>(() => readTransitionOverride(mangaId));
	const transition = transitionOverride ?? globalTransition;
	// A boundary turn with a target chapter first parks on the transition page; a
	// second turn in the same direction (or the page's own continue) jumps
	const [pendingTurn, setPendingTurn] = useState<'next' | 'prev' | null>(null);
	// Per-manga override > manga metadata (viewer) > global default, for mode and direction alike
	const metaMode = modeFromViewer(mangaQuery.data?.viewer);
	const metaDir = dirFromViewer(mangaQuery.data?.viewer);
	const mode = resolveMode(modeOverride, metaMode, globalMode);
	const dir = resolveDir(dirOverride, metaDir, globalDir);
	const [overlay, setOverlay] = useState(true);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [chaptersOpen, setChaptersOpen] = useState(false);
	const drawerBodyRef = useRef<HTMLDivElement>(null);
	// Fullscreen targets the reader root so the toolbars (fixed) stay visible inside it
	const rootRef = useRef<HTMLDivElement>(null);
	const [isFullscreen, setIsFullscreen] = useState(false);
	const fullscreenEnabled = typeof document !== 'undefined' && !!document.fullscreenEnabled;
	useEffect(() => {
		const onFsChange = () => setIsFullscreen(!!document.fullscreenElement);
		document.addEventListener('fullscreenchange', onFsChange);
		return () => document.removeEventListener('fullscreenchange', onFsChange);
	}, []);
	function toggleFullscreen() {
		try {
			if (document.fullscreenElement) void document.exitFullscreen();
			else void rootRef.current?.requestFullscreen();
		} catch {
			// Fullscreen can be denied (permissions policy); the toggle just stays off
		}
	}
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
	const pagedRef = useRef<HTMLDivElement>(null);
	const contentResizeRef = useRef<() => void>(() => {});

	// In paged mode a text page is split into viewport-wide columns, col being the current one; textLayout is the measured column width/gap/count
	const colRef = useRef(0);
	const [col, setColState] = useState(0);
	// Reading-order half of a split page (0 = first); ignored when splitOn is off or the page is not landscape
	const halfRef = useRef<0 | 1>(0);
	const [half, setHalfState] = useState<0 | 1>(0);
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


	const total = pages?.length ?? 0;
	const curPage = pages?.[idx] ?? null;
	const curText = pageText(curPage);
	// Dimensions measured from loaded images fill the gaps the server left (remote
	// chapters carry no dimensions until downloaded). Rebuilding spreads off them
	// keeps idx page-based, so the current page stays put when a mate re-pairs
	const [measuredDims, setMeasuredDims] = useState<Record<number, PageDims>>({});
	function reportDims(arrayIndex: number, width: number, height: number) {
		setMeasuredDims((prev) =>
			recordMeasured(prev, arrayIndex, { width, height }, pageDimensions(pages?.[arrayIndex]))
		);
	}
	function dimsOf(arrayIndex: number): PageDims | null {
		return pageDimensions(pages?.[arrayIndex]) ?? measuredDims[arrayIndex] ?? null;
	}
	// Spreads only exist in paged mode; slots carry page indexes so progress and the
	// slider stay page-based regardless of the layout
	const spreads = useMemo(() => {
		if (mode !== 'paged' || !pages) return null;
		return buildSpreads(
			pages.length,
			(i) => {
				const d = pageDimensions(pages[i]) ?? measuredDims[i] ?? null;
				return d !== null && d.width > d.height;
			},
			layout
		);
	}, [mode, pages, layout, measuredDims]);
	const curSpreadIdx = spreads ? spreadIndexOf(spreads, idx) : 0;
	const curSpread = spreads ? spreads[curSpreadIdx] : null;
	// Wide-page splitting is a paged-mode, single-layout navigation overlay: the
	// current page keeps its real index while a landscape page takes two turns
	const splitOn = mode === 'paged' && layout === 'single' && split;
	function landscapeOf(arrayIndex: number): boolean {
		const d = dimsOf(arrayIndex);
		return d !== null && d.width > d.height;
	}
	// Composition of the visible spread; a re-pair resets stale pinch pan offsets
	const curSpreadKey = curSpread ? curSpread.map((s) => (s === null ? 'b' : s)).join('-') : '';
	// Layout changes leave stale pan offsets behind; curSpreadKey catches a measured
	// re-pair of the visible spread without resetting zoom over distant prefetches
	useLayoutEffect(() => {
		resetPinch();
	}, [mode, idx, col, dir, layout, imgWidth, curSpreadKey, scale, stretch, half, split, resetPinch]);
	// A text chapter is a single page on the server, so its pagination unit is the column, not the page
	const textOnly = mode === 'paged' && total === 1 && pageText(pages?.[0]) !== null;
	// Title/resume use the raw list, so the subtitle and progress survive the current chapter being filtered out
	const curChapter = rawChapters.find((c) => c.id === chapterId) ?? null;
	const pos = navChapters.findIndex((c) => c.id === chapterId);
	const prevCh = pos >= 0 && pos < navChapters.length - 1 ? navChapters[pos + 1] : null;
	const nextCh = pos > 0 ? navChapters[pos - 1] : null;
	const shown = mode === 'paged' ? idx : cur;
	// A split page renders one half of the same image; the box carries the (w/2):h
	// ratio so object-fit: cover overflows by exactly the other half. Unknown-dim
	// pages render whole until recordMeasured proves them landscape
	const splitDims = splitOn && curPage && curText === null && !isUnsupported(curPage) ? dimsOf(idx) : null;
	const splitActive = splitDims !== null && isSplittable(splitDims) ? splitDims : null;
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

	function setHalf(h: 0 | 1) {
		halfRef.current = h;
		setHalfState(h);
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
			const p = pages[center + d];
			if (!p || pageText(p) !== null) continue;
			const im = new Image();
			// The prefetched image doubles as a dimension probe for pages the server has
			// not measured yet. Polling catches the header parse long before load, so
			// even the current page's pairing settles early; the center probe coalesces
			// with the rendered image's request, and onload stays the fallback.
			im.onload = () => reportDims(center + d, im.naturalWidth, im.naturalHeight);
			measureWhenKnown(im, (w, h) => reportDims(center + d, w, h));
			im.src = pageUrl(chapterId, p.index, imgWidth);
		}
	}

	function gotoPage(i: number) {
		const clamped = Math.min(Math.max(i, 0), Math.max(total - 1, 0));
		if (clamped === idxRef.current && pages) return;
		// Moving within the chapter abandons a pending boundary jump
		setPendingTurn(null);
		setIdx(clamped);
		setCol(0);
		setHalf(0);
		if (clamped > furthestRef.current) setFurthest(clamped);
		// Landing on a split final page is not the chapter end yet — its second half is
		scheduleProgress(clamped, clamped >= total - 1 && !(splitOn && landscapeOf(clamped)));
		prefetch(clamped);
	}

	function navCh(c: ChapterWithDownload | null) {
		if (!c) return;
		setPendingTurn(null);
		navigate(`/reader/${c.id}${mangaId !== null ? `?manga=${mangaId}` : ''}`, { replace: true });
	}

	// A boundary turn parks on the transition page when a target chapter exists and
	// the preference is on; the same boundary turn again (or the page's continue
	// action) confirms the jump, and an opposite turn cancels the pending jump
	function boundaryNav(c: ChapterWithDownload | null, direction: 'next' | 'prev') {
		if (!c) return;
		if (pendingTurn === direction) {
			navCh(c);
			return;
		}
		if (pendingTurn !== null) {
			setPendingTurn(null);
			return;
		}
		if (transition) {
			setPendingTurn(direction);
			return;
		}
		navCh(c);
	}

	// Hitting a boundary switches chapters outright: past the last page = next chapter, before the first page = previous chapter
	// Text pages turn column by column first; only when the columns run out does the page/chapter boundary apply
	// Double-page layouts turn spread by spread, landing on each spread's leading page in reading order
	// Split navigation steps through half-pages; text pages keep their column flow
	function turn(forward: boolean) {
		const textPage = curText !== null;
		const cols = textLayoutRef.current.cols;
		if (splitOn && !textPage) {
			const next = turnSplit({ page: idxRef.current, half: halfRef.current }, total, landscapeOf, forward);
			if (next === null) {
				boundaryNav(forward ? nextCh : prevCh, forward ? 'next' : 'prev');
				return;
			}
			if (next.page === idxRef.current) {
				setHalf(next.half);
				scheduleProgress(next.page, splitEndReached(next.page, next.half, total, landscapeOf));
			} else {
				gotoPage(next.page);
				if (next.half === 1) setHalf(1);
			}
			return;
		}
		if (forward) {
			if (textPage && colRef.current < cols - 1) {
				const nextCol = colRef.current + 1;
				setCol(nextCol);
				scheduleProgress(idxRef.current, idxRef.current >= total - 1 && nextCol >= cols - 1);
				return;
			}
			if (spreads) {
				const si = spreadIndexOf(spreads, idxRef.current);
				if (si >= spreads.length - 1) boundaryNav(nextCh, 'next');
				else gotoPage(spreadLeadPage(spreads[si + 1]));
				return;
			}
			if (idxRef.current >= total - 1) boundaryNav(nextCh, 'next');
			else gotoPage(idxRef.current + 1);
		} else {
			if (textPage && colRef.current > 0) {
				setCol(colRef.current - 1);
				scheduleProgress(idxRef.current, false);
				return;
			}
			if (spreads) {
				const si = spreadIndexOf(spreads, idxRef.current);
				if (si <= 0) boundaryNav(prevCh, 'prev');
				else gotoPage(spreadLeadPage(spreads[si - 1]));
				return;
			}
			if (idxRef.current <= 0) boundaryNav(prevCh, 'prev');
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

	function scrollByScreen(sign: 1 | -1) {
		const el = contRef.current;
		if (el) el.scrollBy({ top: sign * el.clientHeight * 0.9, behavior: 'smooth' });
	}

	const keyRef = useRef<(e: KeyboardEvent) => void>(() => {});
	keyRef.current = (e) => {
		if (e.key === 'Escape') {
			// The browser usually consumes the Escape that leaves fullscreen before the
			// page sees it; if it does arrive, leaving fullscreen beats closing panels
			if (document.fullscreenElement) {
				void document.exitFullscreen();
				return;
			}
			if (chaptersOpen) {
				setChaptersOpen(false);
				return;
			}
			if (settingsOpen) {
				setSettingsOpen(false);
				return;
			}
			// A pending chapter jump cancels ahead of leaving the reader
			if (pendingTurn) {
				setPendingTurn(null);
				return;
			}
			goBack();
			return;
		}
		if (chaptersOpen) return;
		const action = readerKeyAction(e, mode);
		if (action === null) return;
		// The key is ours: keep Space and the arrows from scrolling the page natively
		e.preventDefault();
		switch (action) {
			case 'left':
				onZone('left');
				break;
			case 'right':
				onZone('right');
				break;
			case 'pageBack':
				turn(false);
				break;
			case 'pageForward':
				turn(true);
				break;
			case 'scrollBack':
				scrollByScreen(-1);
				break;
			case 'scrollForward':
				scrollByScreen(1);
				break;
			case 'first':
				if (mode === 'paged') gotoPage(0);
				else scrollToPage(0);
				break;
			case 'last':
				if (mode === 'paged') gotoPage(total - 1);
				else scrollToPage(total - 1);
				break;
			case 'toggleOverlay':
				setOverlay((v) => !v);
				break;
			case 'toggleFullscreen':
				toggleFullscreen();
				break;
			case 'cycleScale':
				setScale(nextScale(scale));
				break;
		}
	};
	useEffect(() => {
		const h = (e: KeyboardEvent) => keyRef.current(e);
		window.addEventListener('keydown', h);
		return () => window.removeEventListener('keydown', h);
	}, []);

	// Wheel page turning (paged mode): the controller accumulates deltas, emits one
	// turn per gesture and locks until the wheel goes quiet (trackpad inertia guard)
	const turnRef = useRef(turn);
	turnRef.current = turn;
	const wheelCtlRef = useRef<WheelTurnController | null>(null);
	if (wheelCtlRef.current === null) {
		wheelCtlRef.current = new WheelTurnController({
			setTimeout: (cb, ms) => window.setTimeout(cb, ms),
			clearTimeout: (h) => window.clearTimeout(h as number),
			onTurn: (forward) => turnRef.current(forward)
		});
	}
	const wheelRef = useRef<(e: WheelEvent) => void>(() => {});
	wheelRef.current = (e) => {
		if (mode !== 'paged' || chaptersOpen) return;
		// Scrollable scale modes (width/widthShrink/original) keep the wheel for native scrolling
		if (scale === 'width' || scale === 'widthShrink' || scale === 'original') return;
		// Browser zoom chords and an active pinch zoom keep the wheel to themselves
		if (e.ctrlKey || e.metaKey || zoomed) return;
		// Horizontal gestures (trackpad pans) are not page turns
		if (Math.abs(e.deltaY) < Math.abs(e.deltaX)) return;
		e.preventDefault();
		wheelCtlRef.current?.feed(e.deltaY, e.deltaMode, window.innerHeight);
	};
	useEffect(() => {
		const el = pagedRef.current;
		if (!el) return;
		const h = (e: WheelEvent) => wheelRef.current(e);
		el.addEventListener('wheel', h, { passive: false });
		return () => el.removeEventListener('wheel', h);
	}, [mode, pages]);
	useEffect(() => () => wheelCtlRef.current?.dispose(), []);

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

	function setTapZones(z: TapZoneLayout) {
		if (z === tapZones) return;
		try {
			if (mangaId !== null) localStorage.setItem(tapZonesOverrideKey(mangaId), z);
			else localStorage.setItem('sumidoku.reader.tapzones', z);
		} catch {
			// Private mode blocks persistence but the layout still switches
		}
		setTapZonesOverride(mangaId !== null ? z : null);
	}

	function setBackground(b: ReaderBackground) {
		if (b === bg) return;
		try {
			if (mangaId !== null) localStorage.setItem(backgroundOverrideKey(mangaId), b);
			else localStorage.setItem('sumidoku.reader.bg', b);
		} catch {
			// Private mode blocks persistence but the background still switches
		}
		setBgOverride(mangaId !== null ? b : null);
	}

	function setScale(s: PageScale) {
		if (s === scale) return;
		try {
			if (mangaId !== null) localStorage.setItem(scaleOverrideKey(mangaId), s);
			else localStorage.setItem('sumidoku.reader.scale', s);
		} catch {
			// Private mode blocks persistence but the scale still switches
		}
		setScaleOverride(mangaId !== null ? s : null);
	}

	function setStretch(v: boolean) {
		if (v === stretch) return;
		try {
			if (mangaId !== null) localStorage.setItem(stretchOverrideKey(mangaId), v ? '1' : '0');
			else if (v) localStorage.setItem('sumidoku.reader.stretch', '1');
			else localStorage.removeItem('sumidoku.reader.stretch');
		} catch {
			// Private mode blocks persistence but the stretch still switches
		}
		setStretchOverride(mangaId !== null ? v : null);
	}

	function setSplit(v: boolean) {
		if (v === split) return;
		try {
			if (mangaId !== null) localStorage.setItem(splitOverrideKey(mangaId), v ? '1' : '0');
			else if (v) localStorage.setItem('sumidoku.reader.split', '1');
			else localStorage.removeItem('sumidoku.reader.split');
		} catch {
			// Private mode blocks persistence but the split still switches
		}
		setSplitOverride(mangaId !== null ? v : null);
	}

	function setAnim(v: boolean) {
		if (v === anim) return;
		try {
			if (mangaId !== null) localStorage.setItem(animOverrideKey(mangaId), v ? '1' : '0');
			else if (v) localStorage.removeItem('sumidoku.reader.anim');
			else localStorage.setItem('sumidoku.reader.anim', '0');
		} catch {
			// Private mode blocks persistence but the animation still switches
		}
		setAnimOverride(mangaId !== null ? v : null);
	}

	function setTransition(v: boolean) {
		if (v === transition) return;
		try {
			if (mangaId !== null) localStorage.setItem(transitionOverrideKey(mangaId), v ? '1' : '0');
			else if (v) localStorage.removeItem('sumidoku.reader.transition');
			else localStorage.setItem('sumidoku.reader.transition', '0');
		} catch {
			// Private mode blocks persistence but the transition still switches
		}
		setTransitionOverride(mangaId !== null ? v : null);
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

	// A light tap fires the tap-zone layout's action for its zone; swipes don't
	// trigger it, and while zoomed any tap just toggles the toolbar
	const downRef = useRef({ x: 0, y: 0 });
	function onPagedZoneClick(which: 'left' | 'right' | 'mid') {
		if (consumeClick()) return;
		if (zoomed) {
			setOverlay((v) => !v);
			return;
		}
		const action = zoneAction(dir, tapZones, which);
		if (action === 'menu') setOverlay((v) => !v);
		else if (action === 'next') turn(true);
		else if (action === 'prev') turn(false);
	}
	function onPagedContainerClick(e: RMouseEvent<HTMLDivElement>) {
		if (e.target !== e.currentTarget) return;
		const rect = e.currentTarget.getBoundingClientRect();
		onPagedZoneClick(
			zoneAt(tapZones, (e.clientX - rect.left) / rect.width, (e.clientY - rect.top) / rect.height)
		);
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
	// Canvas background variants: dark stays the unclassed default
	const rootCls = (extra?: string) =>
		['reader', effectiveBg === 'dark' ? '' : `reader--bg-${effectiveBg}`, extra ?? ''].filter(Boolean).join(' ');

	if (pagesQuery.isPending) {
		return (
			<div className={rootCls('reader--msg')}>
				<Spinner />
			</div>
		);
	}
	if (pagesQuery.isError) {
		return (
			<div className={rootCls('reader--msg')}>
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
			<div className={rootCls('reader--msg')}>
				<EmptyState
					icon={BookOpen}
					title={t('reader.noPages')}
					action={<Button onClick={goBack}>{t('reader.back')}</Button>}
				/>
			</div>
		);
	}

	return (
		<div className={rootCls()} ref={rootRef}>
			{mode === 'paged' ? (
				<div
					className="reader__paged"
					data-tz={tapZones}
					data-scale={scale}
					data-stretch={stretch}
					data-anim={anim}
					ref={pagedRef}
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
						{spreads && (
							// Spreads form one sliding track: a turn translates the track by a
							// whole viewport instead of swapping the current page in place
							<div
								className="reader__track"
								style={{ direction: dir, transform: `translateX(${trackOffsetPercent(dir, curSpreadIdx)}%)` }}
							>
								{spreads.map((spread, si) => {
									const isCur = si === curSpreadIdx;
									// The ±2 window keeps both ends of a turn animation mounted; distant
									// spreads render aspect-ratio placeholders instead of the images
									const near = Math.abs(si - curSpreadIdx) <= 2;
									const slotTexts = spread.map((s) => (s !== null && pages?.[s] ? pageText(pages[s]) : null));
									const isTextSpread =
										spread.some((s, k) => s !== null && slotTexts[k] !== null) &&
										spread.every((s, k) => s === null || slotTexts[k] !== null);
									return (
										<div key={si} className="reader__spread" data-slots={spread.length}>
											{isCur && curText !== null ? (
												<div className="reader__spread-inner reader__spread-inner--text">
													<TextColumns
														dir={dir}
														html={md(curText)}
														textStyle={textStyle}
														layout={textLayout}
														col={col}
														innerRef={textColsRef}
													/>
												</div>
											) : isTextSpread ? (
												// Neighbouring text spreads show their first column statically; all
												// spreads share the viewport width, so the measured layout applies
												<div className="reader__spread-inner reader__spread-inner--text">
													<TextColumns
														dir={dir}
														html={md(slotTexts.find((txt) => txt !== null) ?? '')}
														textStyle={textStyle}
														layout={textLayout}
														col={0}
													/>
												</div>
											) : isCur && splitActive ? (
												<div className="reader__spread-inner">
													<div
														className="reader__split"
														style={
															{
																aspectRatio: `${splitActive.width / 2} / ${splitActive.height}`,
																'--split-ratio': `${splitActive.width / 2 / splitActive.height}`,
																'--split-natural-w': `${splitActive.width / 2}px`
															} as CSSProperties
														}
													>
														{/* The half renders the image at twice the box width, so the
														    downsample request doubles to stay sharp */}
														<img
															key={curPage?.index}
															className="reader__img"
															src={pageUrl(chapterId, idx, imgWidth * 2)}
															alt={t('reader.pageAlt', { page: idx + 1 })}
															style={{ objectPosition: `${splitSides(dir)[half]} center` }}
															draggable={false}
															onLoad={(e) =>
																reportDims(idx, e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)
															}
														/>
													</div>
												</div>
											) : (
												// Slots are in reading order; the row's direction puts the first
												// slot on the leading side (right under rtl)
												<div className="reader__spread-inner">
													{spread.map((slot, k) => {
														if (slot === null || !pages?.[slot]) {
															// A blank takes its mate page's aspect ratio (KMReader's
															// blankOf convention), so the visible page sits exactly
															// where the joined pair would place it
															const mate = spread[k === 0 ? 1 : 0];
															const mateDims = mate !== null && mate !== undefined ? dimsOf(mate) : null;
															const ratio = slotAspectRatio(mateDims);
															return (
																<div
																	key={`blank-${k}`}
																	className="reader__blank"
																	style={{ aspectRatio: ratio }}
																/>
															);
														}
														const p = pages[slot];
														const slotText = pageText(p);
														const dims = dimsOf(slot);
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
														if (!near) {
															return (
																<div
																	key={p.index}
																	className="reader__img"
																	style={{ aspectRatio: slotAspectRatio(dims), width: '100%' }}
																	aria-hidden
																/>
															);
														}
														return (
															<img
																key={p.index}
																className="reader__img"
																src={pageUrl(chapterId, p.index, imgWidth)}
																alt={t('reader.pageAlt', { page: slot + 1 })}
																// The slot keeps an aspect-ratio box even before dims are
																// known (2:3 fallback), so an unloaded page stays in its own
																// half instead of collapsing to zero size and letting the
																// loaded mate sit in the middle of the spread
																style={{ aspectRatio: slotAspectRatio(dims) }}
																draggable={false}
																onLoad={(e) =>
																	reportDims(slot, e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)
																}
															/>
														);
													})}
												</div>
											)}
										</div>
									);
								})}
							</div>
						)}
					</div>
					{tapZones !== 'off' && (
						<>
							<button
								type="button"
								className="reader__zone reader__zone--left"
								aria-label={t(zoneAction(dir, tapZones, 'left') === 'next' ? 'reader.nextPage' : 'reader.prevPage')}
								onClick={(e) => {
									e.stopPropagation();
									onPagedZoneClick('left');
								}}
							/>
							<button
								type="button"
								className="reader__zone reader__zone--right"
								aria-label={t(zoneAction(dir, tapZones, 'right') === 'next' ? 'reader.nextPage' : 'reader.prevPage')}
								onClick={(e) => {
									e.stopPropagation();
									onPagedZoneClick('right');
								}}
							/>
							<button
								type="button"
								className="reader__zone reader__zone--mid"
								aria-label={t('reader.toggleToolbar')}
								onClick={(e) => {
									e.stopPropagation();
									onPagedZoneClick('mid');
								}}
							/>
						</>
					)}
					{pendingTurn && (
						<TransitionPage
							direction={pendingTurn}
							currentLabel={curChapter ? chapterLabel(curChapter) : mangaTitle}
							target={pendingTurn === 'next' ? nextCh : prevCh}
							onContinue={() => {
								const target = pendingTurn === 'next' ? nextCh : prevCh;
								setPendingTurn(null);
								navCh(target);
							}}
							onCancel={() => setPendingTurn(null)}
						/>
					)}
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
							{pages.map((p, arrayIndex) => {
								const text = pageText(p);
								const unsupported = isUnsupported(p);
								const dimensions = text === null && !unsupported ? dimsOf(arrayIndex) : null;
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
												onLoad={(e) => {
													contentResizeRef.current();
													reportDims(arrayIndex, e.currentTarget.naturalWidth, e.currentTarget.naturalHeight);
												}}
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
				{fullscreenEnabled && (
					<IconButton
						label={isFullscreen ? t('reader.exitFullscreen') : t('reader.enterFullscreen')}
						className={isFullscreen ? 'reader__icon--on' : undefined}
						onClick={toggleFullscreen}
					>
						{isFullscreen ? <Minimize size={17} /> : <Maximize size={17} />}
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
					tapZones={tapZones}
					onTapZonesChange={setTapZones}
					bg={bg}
					onBgChange={setBackground}
					scale={scale}
					onScaleChange={setScale}
					split={split}
					onSplitChange={setSplit}
					stretch={stretch}
					onStretchChange={setStretch}
					anim={anim}
					onAnimChange={setAnim}
					transition={transition}
					onTransitionChange={setTransition}
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
