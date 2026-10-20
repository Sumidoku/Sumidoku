import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, PointerEvent as RPointerEvent } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { marked } from 'marked';
import {
	AlignJustify,
	ArrowLeft,
	BookOpen,
	Settings,
	StretchVertical,
	TriangleAlert
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { api, pageUrl } from '../api/client';
import { qk, useChapterPages, useManga, useMangaChapters } from '../api/hooks';
import type { Chapter, PageData } from '../api/types';
import { chapterLabel } from '../lib/format';
import { applyPrefs, DEFAULT_PREFS, loadPrefs } from '../lib/chapterPrefs';
import { Button, EmptyState, IconButton, SegmentedControl, Spinner } from '../components';
import './reader.css';

// continuous is the stored value for "scroll" in the old localStorage; renaming it would lose existing users' settings
type Mode = 'paged' | 'continuous' | 'webtoon';
type Dir = 'ltr' | 'rtl';

const MODES: { key: Mode; label: string; icon: LucideIcon }[] = [
	{ key: 'paged', label: '翻页', icon: BookOpen },
	{ key: 'continuous', label: '滚动', icon: AlignJustify },
	{ key: 'webtoon', label: '条漫', icon: StretchVertical }
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

function isText(p: PageData | undefined | null): p is PageData & { content: { Text: string } } {
	return !!p?.content && typeof p.content === 'object' && 'Text' in p.content;
}

/** The server 501s the Image variant outright, and the frontend has no way to render it either */
function isUnsupported(p: PageData): boolean {
	return !!p.content && typeof p.content === 'object' && 'Image' in p.content;
}

// A hasDescription page displays its description (already fetched by the server); other text pages use content.Text
function pageText(p: PageData | undefined | null): string | null {
	if (!p) return null;
	if (p.hasDescription && typeof p.description === 'string') return p.description;
	if (isText(p)) return p.content.Text;
	return null;
}

// Keep the old trust model: markdown output is not sanitized
function md(text: string): string {
	return marked.parse(text, { async: false }) as string;
}

// Page-image downsampling target width = display container CSS width × DPR, bucketed up to 256 by the server; webtoon/paged fill the screen width, continuous caps at 780
function readImgWidth(mode: Mode): number {
	const css = mode === 'continuous' ? Math.min(window.innerWidth, 780) : window.innerWidth;
	return Math.max(1, Math.round(css * (window.devicePixelRatio || 1)));
}

export default function ReaderPage() {
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
				<EmptyState icon={TriangleAlert} title="章节地址无效" />
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

	const pagesQuery = useChapterPages(chapterId);
	const pages = useMemo(() => pagesQuery.data ?? null, [pagesQuery.data]);

	const navEnabled = mangaId !== null;
	const mangaQuery = useManga(mangaId ?? 0, { enabled: navEnabled, silent: true });
	const chaptersQuery = useMangaChapters(mangaId ?? 0, { enabled: navEnabled, silent: true });
	// The raw list is kept for chapter titles/resume progress; the sorted/filtered list only drives prev/next-chapter navigation
	const rawChapters = useMemo(() => chaptersQuery.data ?? [], [chaptersQuery.data]);
	const prefs = useMemo(() => (mangaId !== null ? loadPrefs(mangaId) : DEFAULT_PREFS), [mangaId]);
	const chapters = useMemo(() => applyPrefs(rawChapters, prefs), [rawChapters, prefs]);
	const navChapters = useMemo(() => (prefs.asc ? [...chapters].reverse() : chapters), [chapters, prefs.asc]);

	const [mode, setModeState] = useState<Mode>(readMode);
	const [dir, setDirState] = useState<Dir>(readDir);
	const [overlay, setOverlay] = useState(true);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [imgWidth, setImgWidth] = useState(() => readImgWidth(mode));

	const idxRef = useRef(0);
	const [idx, setIdxState] = useState(0);
	const furthestRef = useRef(0);
	// In continuous/webtoon mode, the page at the viewport top, used for the page display; furthest is only for progress reporting
	const curRef = useRef(0);
	const [cur, setCurState] = useState(0);
	const initedRef = useRef(false);
	const contRef = useRef<HTMLDivElement>(null);

	// In paged mode a text page is split into viewport-wide columns, col being the current one; textLayout is the measured column width/gap/count
	const colRef = useRef(0);
	const [col, setColState] = useState(0);
	const [textLayout, setTextLayout] = useState({ w: 0, gap: 0, cols: 1 });
	const textLayoutRef = useRef(textLayout);
	textLayoutRef.current = textLayout;
	const textColsRef = useRef<HTMLDivElement>(null);

	const total = pages?.length ?? 0;
	const curPage = pages?.[idx] ?? null;
	const curText = pageText(curPage);
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
					qc.setQueryData<Chapter[]>(qk.chapters(mangaId), (old) =>
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

	function navCh(c: Chapter | null) {
		if (!c) return;
		navigate(`/reader/${c.id}${mangaId !== null ? `?manga=${mangaId}` : ''}`);
	}

	// Hitting a boundary switches chapters outright: past the last page = next chapter, before the first page = previous chapter
	// Text pages turn column by column first; only when the columns run out does the page/chapter boundary apply
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
			if (idxRef.current >= total - 1) navCh(nextCh);
			else gotoPage(idxRef.current + 1);
		} else {
			if (textPage && colRef.current > 0) {
				setCol(colRef.current - 1);
				scheduleProgress(idxRef.current, false);
				return;
			}
			if (idxRef.current <= 0) navCh(prevCh);
			else gotoPage(idxRef.current - 1);
		}
	}

	function onZone(which: 'left' | 'right') {
		turn(dir === 'ltr' ? which === 'right' : which === 'left');
	}

	function goBack() {
		if (mangaId !== null) navigate(`/manga/${mangaId}`);
		else navigate(-1);
	}

	const keyRef = useRef<(e: KeyboardEvent) => void>(() => {});
	keyRef.current = (e) => {
		if (e.key === 'Escape') {
			goBack();
			return;
		}
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
		document.title = mangaTitle ? `${mangaTitle} · 阅读 - Sumidoku` : '阅读 - Sumidoku';
		return () => {
			document.title = 'Sumidoku';
		};
	}, [mangaTitle]);

	function setMode(m: Mode) {
		if (m === mode) return;
		setModeState(m);
		try {
			localStorage.setItem('sumidoku.reader.mode', m);
		} catch {
			// Private mode blocks persistence but the mode still switches
		}
	}

	function setDir(d: Dir) {
		setDirState(d);
		try {
			localStorage.setItem('sumidoku.reader.dir', d);
		} catch {
			// Same as above
		}
	}

	function scrollToPage(i: number) {
		contRef.current
			?.querySelector(`[data-page="${i}"]`)
			?.scrollIntoView({ block: 'start', behavior: 'instant' });
	}

	// Start page: ?page > chapter reading progress > 0; don't move before chapter navigation data is in place, to avoid landing on the wrong progress
	const navReady = !navEnabled || chaptersQuery.isFetched;
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
	}, [mode, idx, pages]);

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
		update();
		window.addEventListener('scroll', onScroll, { passive: true });
		// Lazy-loaded images change the container height, so recompute the current page with a ResizeObserver
		const ro = new ResizeObserver(onScroll);
		ro.observe(root);
		return () => {
			window.removeEventListener('scroll', onScroll);
			ro.disconnect();
			if (raf) cancelAnimationFrame(raf);
		};
	}, [mode, pages, navReady]);

	// A light tap in the middle summons the toolbar; swipes don't trigger it
	const downRef = useRef({ x: 0, y: 0 });
	function onPointerDown(e: RPointerEvent) {
		downRef.current = { x: e.clientX, y: e.clientY };
	}
	function onPointerUp(e: RPointerEvent) {
		if (Math.hypot(e.clientX - downRef.current.x, e.clientY - downRef.current.y) > 10) return;
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
					title="章节加载失败"
					hint={pagesQuery.error?.message}
					action={
						<Button variant="primary" onClick={() => void pagesQuery.refetch()}>
							重试
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
					title="本章没有页面"
					action={<Button onClick={goBack}>返回</Button>}
				/>
			</div>
		);
	}

	return (
		<div className="reader">
			{mode === 'paged' ? (
				<div className="reader__paged">
					{curPage &&
						(curText !== null ? (
							<div className="reader__text-paged">
								<div
									className="reader-text reader-text--cols"
									ref={textColsRef}
									style={{
										columnWidth: textLayout.w > 0 ? `${textLayout.w}px` : undefined,
										transform: `translateX(${-col * (textLayout.w + textLayout.gap)}px)`
									}}
									dangerouslySetInnerHTML={{ __html: md(curText) }}
								/>
							</div>
						) : isUnsupported(curPage) ? (
							<div className="reader__unsupported">该页类型暂不支持</div>
						) : (
							<img
								className="reader__img"
								src={pageUrl(chapterId, curPage.index, imgWidth)}
								alt={`第 ${idx + 1} 页`}
							/>
						))}
					<button
						type="button"
						className="reader__zone reader__zone--left"
						aria-label="上一页"
						onClick={() => onZone('left')}
					/>
					<button
						type="button"
						className="reader__zone reader__zone--right"
						aria-label="下一页"
						onClick={() => onZone('right')}
					/>
					<button
						type="button"
						className="reader__zone reader__zone--mid"
						aria-label="显示或隐藏工具栏"
						onClick={() => setOverlay((v) => !v)}
					/>
				</div>
			) : (
				<div
					className={mode === 'webtoon' ? 'reader__cont reader__cont--webtoon' : 'reader__cont'}
					ref={contRef}
					onPointerDown={onPointerDown}
					onPointerUp={onPointerUp}
				>
					{pages.map((p) => {
						const text = pageText(p);
						return (
							<div className="reader__pg" data-page={p.index} key={p.index}>
								{text !== null ? (
									<div className="reader-text" dangerouslySetInnerHTML={{ __html: md(text) }} />
								) : isUnsupported(p) ? (
									<div className="reader__unsupported">该页类型暂不支持</div>
								) : (
									<img src={pageUrl(chapterId, p.index, imgWidth)} alt={`第 ${p.index + 1} 页`} loading="lazy" />
								)}
							</div>
						);
					})}
				</div>
			)}

			<div className={barCls('reader__bar reader__bar--top')}>
				<IconButton label="返回" onClick={goBack}>
					<ArrowLeft size={18} />
				</IconButton>
				<div className="reader__titles">
					<div className="reader__title">{mangaTitle || '阅读'}</div>
					{curChapter && <div className="reader__subtitle">{chapterLabel(curChapter)}</div>}
				</div>
				<SegmentedControl
					options={MODES.map((m) => ({
						value: m.key,
						label: (
							<span className="reader__mode" title={m.label}>
								<m.icon size={15} />
								<span className="reader__mode-label">{m.label}</span>
							</span>
						)
					}))}
					value={mode}
					onChange={setMode}
				/>
				<IconButton
					label="阅读设置"
					className={settingsOpen ? 'reader__icon--on' : undefined}
					onClick={() => setSettingsOpen((v) => !v)}
				>
					<Settings size={17} />
				</IconButton>
			</div>

			{overlay && settingsOpen && (
				<div className="reader__settings card">
					<div className="reader__settings-title">
						阅读方向{mode !== 'paged' ? '（仅翻页模式）' : ''}
					</div>
					<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
						<SegmentedControl
							options={[
								{ value: 'ltr', label: '从左到右' },
								{ value: 'rtl', label: '从右到左' }
							]}
							value={dir}
							onChange={setDir}
						/>
					</div>
				</div>
			)}

			<div className={barCls('reader__bar reader__bar--bottom')}>
				<Button size="sm" disabled={!prevCh} onClick={() => navCh(prevCh)}>
					上一章
				</Button>
				<div className="reader__pager">
					<input
						type="range"
						min={1}
						max={navTotal}
						value={Math.min(navPos + 1, navTotal)}
						onChange={onSlider}
						aria-label="跳转到指定页"
					/>
					<span className="reader__page-num">
						{Math.min(navPos + 1, navTotal)}/{navTotal}
					</span>
				</div>
				<Button size="sm" disabled={!nextCh} onClick={() => navCh(nextCh)}>
					下一章
				</Button>
			</div>
		</div>
	);
}
