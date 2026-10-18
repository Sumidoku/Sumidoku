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

// 沿用旧版信任模型，markdown 结果不做 sanitize
function md(text: string): string {
	return marked.parse(text, { async: false }) as string;
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

	const idxRef = useRef(0);
	const [idx, setIdxState] = useState(0);
	const furthestRef = useRef(0);
	const [furthest, setFurthestState] = useState(0);
	const initedRef = useRef(false);
	const contRef = useRef<HTMLDivElement>(null);

	const total = pages?.length ?? 0;
	const curPage = pages?.[idx] ?? null;
	// 标题/续读用原列表，避免当前章被筛掉时丢了副标题和进度
	const curChapter = rawChapters.find((c) => c.id === chapterId) ?? null;
	const pos = chapters.findIndex((c) => c.id === chapterId);
	const prevCh = pos > 0 ? chapters[pos - 1] : null;
	const nextCh = pos >= 0 && pos < chapters.length - 1 ? chapters[pos + 1] : null;
	const shown = mode === 'paged' ? idx : furthest;
	const mangaTitle = mangaQuery.data?.title ?? '';

	function setIdx(i: number) {
		idxRef.current = i;
		setIdxState(i);
	}

	function setFurthest(i: number) {
		furthestRef.current = i;
		setFurthestState(i);
	}

	const reportRef = useRef<{
		timer: ReturnType<typeof setTimeout> | undefined;
		pending: number | null;
		readReported: boolean;
	}>({ timer: undefined, pending: null, readReported: false });

	// 进度上报防抖 300ms，失败静默不打断阅读；到最后一页顺带标已读，一章只标一次
	function flushProgress() {
		const r = reportRef.current;
		if (r.timer !== undefined) {
			clearTimeout(r.timer);
			r.timer = undefined;
		}
		if (r.pending === null) return;
		const i = r.pending;
		r.pending = null;
		const body: { lastPageRead: number; isRead?: boolean } = { lastPageRead: i };
		const markRead = total > 0 && i >= total - 1 && !r.readReported;
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

	function scheduleProgress(i: number) {
		const r = reportRef.current;
		r.pending = i;
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
			if (!p || isText(p)) continue;
			const im = new Image();
			im.src = pageUrl(chapterId, p.index);
		}
	}

	function gotoPage(i: number) {
		const clamped = Math.min(Math.max(i, 0), Math.max(total - 1, 0));
		if (clamped === idxRef.current && pages) return;
		setIdx(clamped);
		if (clamped > furthestRef.current) setFurthest(clamped);
		scheduleProgress(clamped);
		prefetch(clamped);
	}

	function navCh(c: Chapter | null) {
		if (!c) return;
		navigate(`/reader/${c.id}${mangaId !== null ? `?manga=${mangaId}` : ''}`);
	}

	// 翻到边界直接换章：最后一页再往前 = 下一章，第一页再往回 = 上一章
	function turn(forward: boolean) {
		if (forward) {
			if (idxRef.current >= total - 1) navCh(nextCh);
			else gotoPage(idxRef.current + 1);
		} else if (idxRef.current <= 0) {
			navCh(prevCh);
		} else {
			gotoPage(idxRef.current - 1);
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
		prefetch(initial);
		if (mode !== 'paged') scrollToPage(initial);
	});

	// 切到滚动/条漫后定位回当前页
	const prevModeRef = useRef(mode);
	useLayoutEffect(() => {
		if (prevModeRef.current === mode) return;
		prevModeRef.current = mode;
		if (mode !== 'paged' && initedRef.current) scrollToPage(idxRef.current);
	}, [mode]);

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
					scheduleProgress(i);
				}
			}
		});
		els.forEach((el) => ob.observe(el));
		return () => ob.disconnect();
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
			gotoPage(i);
			return;
		}
		scrollToPage(i);
		setIdx(i);
		if (i > furthestRef.current) {
			setFurthest(i);
			scheduleProgress(i);
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
						(isText(curPage) ? (
							<div className="reader__text-scroll">
								<div className="reader-text" dangerouslySetInnerHTML={{ __html: md(curPage.content.Text) }} />
							</div>
						) : isUnsupported(curPage) ? (
							<div className="reader__unsupported">该页类型暂不支持</div>
						) : (
							<img
								className="reader__img"
								src={pageUrl(chapterId, curPage.index)}
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
					{pages.map((p) => (
						<div className="reader__pg" data-page={p.index} key={p.index}>
							{isText(p) ? (
								<div className="reader-text" dangerouslySetInnerHTML={{ __html: md(p.content.Text) }} />
							) : isUnsupported(p) ? (
								<div className="reader__unsupported">该页类型暂不支持</div>
							) : (
								<img src={pageUrl(chapterId, p.index)} alt={`第 ${p.index + 1} 页`} loading="lazy" />
							)}
						</div>
					))}
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
						max={total}
						value={Math.min(shown + 1, total)}
						onChange={onSlider}
						aria-label="跳转到指定页"
					/>
					<span className="reader__page-num">
						{Math.min(shown + 1, total)}/{total}
					</span>
				</div>
				<Button size="sm" disabled={!nextCh} onClick={() => navCh(nextCh)}>
					下一章
				</Button>
			</div>
		</div>
	);
}
