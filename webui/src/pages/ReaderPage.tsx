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

// continuous 是旧版 localStorage 里「滚动」的存值，改名会丢老用户设置
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
		// 旧版设置页把「滚动」误存为 vertical，等价成 continuous
		if (m === 'vertical') return 'continuous';
	} catch {
		// 隐私模式读不到存储就用默认
	}
	return 'paged';
}

function readDir(): Dir {
	try {
		if (localStorage.getItem('sumidoku.reader.dir') === 'rtl') return 'rtl';
	} catch {
		// 同上
	}
	return 'ltr';
}

function isText(p: PageData | undefined | null): p is PageData & { content: { Text: string } } {
	return !!p?.content && typeof p.content === 'object' && 'Text' in p.content;
}

/** Image 变体服务端直接 501，前端也没有渲染途径 */
function isUnsupported(p: PageData): boolean {
	return !!p.content && typeof p.content === 'object' && 'Image' in p.content;
}

// hasDescription 页的展示文本是 description（服务端已回源取好），其余文本页走 content.Text
function pageText(p: PageData | undefined | null): string | null {
	if (!p) return null;
	if (p.hasDescription && typeof p.description === 'string') return p.description;
	if (isText(p)) return p.content.Text;
	return null;
}

// 沿用旧版信任模型，markdown 结果不做 sanitize
function md(text: string): string {
	return marked.parse(text, { async: false }) as string;
}

// 页图降采样的目标宽 = 显示容器 CSS 宽 × DPR，服务端再按 256 向上取桶；条漫/翻页铺满屏宽，滚动栏上限 780
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
	// key 钉在 chapterId 上，切章整体重挂载，页码/上报队列/observer 随之重置
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
	// 原列表保留给章节标题/续读进度；排序/筛选后的列表只用于上一章/下一章导航
	const rawChapters = useMemo(() => chaptersQuery.data ?? [], [chaptersQuery.data]);
	const prefs = useMemo(() => (mangaId !== null ? loadPrefs(mangaId) : DEFAULT_PREFS), [mangaId]);
	const chapters = useMemo(() => applyPrefs(rawChapters, prefs), [rawChapters, prefs]);

	const [mode, setModeState] = useState<Mode>(readMode);
	const [dir, setDirState] = useState<Dir>(readDir);
	const [overlay, setOverlay] = useState(true);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const [imgWidth, setImgWidth] = useState(() => readImgWidth(mode));

	const idxRef = useRef(0);
	const [idx, setIdxState] = useState(0);
	const furthestRef = useRef(0);
	// 滚动/条漫模式下视口顶部所在的页，用于页码显示；furthest 只用于进度上报
	const curRef = useRef(0);
	const [cur, setCurState] = useState(0);
	const initedRef = useRef(false);
	const contRef = useRef<HTMLDivElement>(null);

	// 文本页在翻页模式下按视口切成多栏，col 是当前栏；textLayout 是实测的栏宽/间距/栏数
	const colRef = useRef(0);
	const [col, setColState] = useState(0);
	const [textLayout, setTextLayout] = useState({ w: 0, gap: 0, cols: 1 });
	const textLayoutRef = useRef(textLayout);
	textLayoutRef.current = textLayout;
	const textColsRef = useRef<HTMLDivElement>(null);

	const total = pages?.length ?? 0;
	const curPage = pages?.[idx] ?? null;
	const curText = pageText(curPage);
	// 文本章在服务端就是单页，这种章的分页单位是栏而不是页
	const textOnly = mode === 'paged' && total === 1 && pageText(pages?.[0]) !== null;
	// 标题/续读用原列表，避免当前章被筛掉时丢了副标题和进度
	const curChapter = rawChapters.find((c) => c.id === chapterId) ?? null;
	const pos = chapters.findIndex((c) => c.id === chapterId);
	const prevCh = pos > 0 ? chapters[pos - 1] : null;
	const nextCh = pos >= 0 && pos < chapters.length - 1 ? chapters[pos + 1] : null;
	const shown = mode === 'paged' ? idx : cur;
	// 文本单页章的分页单位是栏，其余是页
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

	// 窗口尺寸变化后按新宽度重新取变体，防抖避免拖拽缩放期间反复拉图
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

	// 进度上报防抖 300ms，失败静默不打断阅读；到章节末尾顺带标已读，一章只标一次
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

	// 退出时防抖队列里可能还有未上报进度，先补发再卸载
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

	// 翻到边界直接换章：最后一页再往前 = 下一章，第一页再往回 = 上一章
	// 文本页先逐栏翻，栏翻完再走页/章的边界
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
			// 隐私模式不持久化也照常切换
		}
	}

	function setDir(d: Dir) {
		setDirState(d);
		try {
			localStorage.setItem('sumidoku.reader.dir', d);
		} catch {
			// 同上
		}
	}

	function scrollToPage(i: number) {
		contRef.current
			?.querySelector(`[data-page="${i}"]`)
			?.scrollIntoView({ block: 'start', behavior: 'instant' });
	}

	// 起始页：?page > 章节阅读进度 > 0；章节导航数据没就位前不动，免得按错的进度定位
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

	// 切到滚动/条漫后定位回当前页
	const prevModeRef = useRef(mode);
	useLayoutEffect(() => {
		if (prevModeRef.current === mode) return;
		prevModeRef.current = mode;
		if (mode === 'paged') {
			// 回到翻页时对齐到滚动模式当前页，而不是离开时停留的 idx
			setIdx(curRef.current);
			setCol(0);
		} else {
			// 进入滚动/条漫时，页码回到翻页模式当前页，而不是沿用历史最远页
			setCur(idxRef.current);
			if (initedRef.current) scrollToPage(idxRef.current);
		}
	}, [mode]);

	// 翻页模式下把文本页按视口切成等宽多栏；实测栏数用于分页和小节跳转
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

	// 滚动/条漫模式进度 = 进入过视口的最大页码
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

	// 滚动/条漫模式页码 = 覆盖视口顶部的那一页，随滚动实时更新
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
		// 图片懒加载会改变容器高度，用 ResizeObserver 重算当前页
		const ro = new ResizeObserver(onScroll);
		ro.observe(root);
		return () => {
			window.removeEventListener('scroll', onScroll);
			ro.disconnect();
			if (raf) cancelAnimationFrame(raf);
		};
	}, [mode, pages, navReady]);

	// 轻点中部呼出工具栏，滑动不触发
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
