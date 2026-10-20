<script lang="ts">
	import { onMount, onDestroy, untrack } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { marked } from 'marked';
	import AlignJustify from '@lucide/svelte/icons/align-justify';
	import ArrowLeft from '@lucide/svelte/icons/arrow-left';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import Settings from '@lucide/svelte/icons/settings';
	import StretchVertical from '@lucide/svelte/icons/stretch-vertical';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { api, pageUrl } from '$lib/api/client';
	import { qk, queryClient, useChapterPages, useManga, useMangaChapters } from '$lib/api/hooks';
	import type { Chapter, PageData } from '$lib/api/types';
	import { chapterLabel } from '$lib/format';
	import { applyPrefs, DEFAULT_PREFS, loadPrefs } from '$lib/chapterPrefs';
	import { Button, EmptyState, IconButton, SegmentedControl, Spinner } from '$lib/components';

	// continuous 是旧版 localStorage 里「滚动」的存值，改名会丢老用户设置
	type Mode = 'paged' | 'continuous' | 'webtoon';
	type Dir = 'ltr' | 'rtl';

	let {
		chapterId,
		mangaId,
		startPage
	}: { chapterId: number; mangaId: number | null; startPage: number | null } = $props();

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
	function readImgWidth(m: Mode): number {
		const css = m === 'continuous' ? Math.min(window.innerWidth, 780) : window.innerWidth;
		return Math.max(1, Math.round(css * (window.devicePixelRatio || 1)));
	}

	const pagesQuery = useChapterPages(() => chapterId);
	const pages = $derived(pagesQuery.data ?? null);

	const navEnabled = $derived(mangaId !== null);
	const mangaQuery = useManga(
		() => mangaId ?? 0,
		() => ({ enabled: navEnabled, silent: true })
	);
	const chaptersQuery = useMangaChapters(
		() => mangaId ?? 0,
		() => ({ enabled: navEnabled, silent: true })
	);
	// 原列表保留给章节标题/续读进度；排序/筛选后的列表只用于上一章/下一章导航
	const rawChapters = $derived(chaptersQuery.data ?? []);
	const prefs = $derived(mangaId !== null ? loadPrefs(mangaId) : DEFAULT_PREFS);
	const chapters = $derived(applyPrefs(rawChapters, prefs));

	let mode = $state<Mode>(readMode());
	let dir = $state<Dir>(readDir());
	let overlay = $state(true);
	let settingsOpen = $state(false);
	let imgWidth = $state(readImgWidth(readMode()));

	// idx/cur/col 既驱动渲染又被事件处理器同步读取；furthest 只参与进度上报不进渲染
	let idx = $state(0);
	let furthest = 0;
	// 滚动/条漫模式下视口顶部所在的页，用于页码显示；furthest 只用于进度上报
	let cur = $state(0);
	let inited = false;
	let contEl = $state<HTMLDivElement>();

	// 文本页在翻页模式下按视口切成多栏，col 是当前栏；textLayout 是实测的栏宽/间距/栏数
	let col = $state(0);
	let textLayout = $state({ w: 0, gap: 0, cols: 1 });
	let textColsEl = $state<HTMLDivElement>();

	const total = $derived(pages?.length ?? 0);
	const curPage = $derived(pages?.[idx] ?? null);
	const curText = $derived(pageText(curPage));
	// 文本章在服务端就是单页，这种章的分页单位是栏而不是页
	const textOnly = $derived(mode === 'paged' && total === 1 && pageText(pages?.[0]) !== null);
	// 标题/续读用原列表，避免当前章被筛掉时丢了副标题和进度
	const curChapter = $derived(rawChapters.find((c) => c.id === chapterId) ?? null);
	const pos = $derived(chapters.findIndex((c) => c.id === chapterId));
	const prevCh = $derived(pos > 0 ? chapters[pos - 1] : null);
	const nextCh = $derived(pos >= 0 && pos < chapters.length - 1 ? chapters[pos + 1] : null);
	const shown = $derived(mode === 'paged' ? idx : cur);
	// 文本单页章的分页单位是栏，其余是页
	const navTotal = $derived(textOnly ? textLayout.cols : total);
	const navPos = $derived(textOnly ? col : shown);
	const mangaTitle = $derived(mangaQuery.data?.title ?? '');

	const textColsStyle = $derived(
		`${textLayout.w > 0 ? `column-width:${textLayout.w}px;` : ''}transform:translateX(${
			-col * (textLayout.w + textLayout.gap)
		}px)`
	);

	// 窗口尺寸变化后按新宽度重新取变体，防抖避免拖拽缩放期间反复拉图
	$effect(() => {
		const m = mode;
		imgWidth = readImgWidth(m);
		let timer: ReturnType<typeof setTimeout> | undefined;
		const onResize = () => {
			if (timer !== undefined) clearTimeout(timer);
			timer = setTimeout(() => (imgWidth = readImgWidth(m)), 200);
		};
		window.addEventListener('resize', onResize);
		return () => {
			if (timer !== undefined) clearTimeout(timer);
			window.removeEventListener('resize', onResize);
		};
	});

	const report: {
		timer: ReturnType<typeof setTimeout> | undefined;
		pending: { page: number; atEnd: boolean } | null;
		readReported: boolean;
	} = { timer: undefined, pending: null, readReported: false };

	// 进度上报防抖 300ms，失败静默不打断阅读；到章节末尾顺带标已读，一章只标一次
	function flushProgress() {
		if (report.timer !== undefined) {
			clearTimeout(report.timer);
			report.timer = undefined;
		}
		if (report.pending === null) return;
		const { page: i, atEnd } = report.pending;
		report.pending = null;
		const body: { lastPageRead: number; isRead?: boolean } = { lastPageRead: i };
		const markRead = total > 0 && atEnd && !report.readReported;
		if (markRead) body.isRead = true;
		api(`/chapter/${chapterId}`, { method: 'PATCH', body, silent: true })
			.then(() => {
				if (markRead) report.readReported = true;
				if (mangaId !== null) {
					queryClient.setQueryData<Chapter[]>(qk.chapters(mangaId), (old) =>
						old?.map((c) =>
							c.id === chapterId
								? { ...c, lastPageRead: i, isRead: markRead ? true : c.isRead }
								: c
						)
					);
				}
			})
			.catch(() => {});
	}

	function scheduleProgress(i: number, atEnd: boolean) {
		report.pending = { page: i, atEnd };
		if (report.timer !== undefined) clearTimeout(report.timer);
		report.timer = setTimeout(flushProgress, 300);
	}

	// 退出时防抖队列里可能还有未上报进度，先补发再卸载
	onDestroy(() => flushProgress());

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
		if (clamped === idx && pages) return;
		idx = clamped;
		col = 0;
		if (clamped > furthest) furthest = clamped;
		scheduleProgress(clamped, clamped >= total - 1);
		prefetch(clamped);
	}

	function navCh(c: Chapter | null) {
		if (!c) return;
		goto(`/reader/${c.id}${mangaId !== null ? `?manga=${mangaId}` : ''}`);
	}

	// 翻到边界直接换章：最后一页再往前 = 下一章，第一页再往回 = 上一章
	// 文本页先逐栏翻，栏翻完再走页/章的边界
	function turn(forward: boolean) {
		const textPage = curText !== null;
		const cols = textLayout.cols;
		if (forward) {
			if (textPage && col < cols - 1) {
				const nextCol = col + 1;
				col = nextCol;
				scheduleProgress(idx, idx >= total - 1 && nextCol >= cols - 1);
				return;
			}
			if (idx >= total - 1) navCh(nextCh);
			else gotoPage(idx + 1);
		} else {
			if (textPage && col > 0) {
				col = col - 1;
				scheduleProgress(idx, false);
				return;
			}
			if (idx <= 0) navCh(prevCh);
			else gotoPage(idx - 1);
		}
	}

	function onZone(which: 'left' | 'right') {
		turn(dir === 'ltr' ? which === 'right' : which === 'left');
	}

	function goBack() {
		if (mangaId !== null) goto(`/manga/${mangaId}`);
		else window.history.back();
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			goBack();
			return;
		}
		if (mode !== 'paged') return;
		if (e.key === 'ArrowLeft') onZone('left');
		else if (e.key === 'ArrowRight') onZone('right');
	}

	onMount(() => {
		window.addEventListener('keydown', onKey);
		return () => window.removeEventListener('keydown', onKey);
	});

	// 标签标题跟随书名变化，离开阅读器时还原
	$effect(() => {
		document.title = mangaTitle
			? $_('reader.docTitle', { values: { title: mangaTitle } })
			: $_('reader.docTitleBare');
		return () => {
			document.title = 'Sumidoku';
		};
	});

	function setMode(m: Mode) {
		if (m === mode) return;
		mode = m;
		try {
			localStorage.setItem('sumidoku.reader.mode', m);
		} catch {
			// 隐私模式不持久化也照常切换
		}
	}

	function setDir(d: Dir) {
		dir = d;
		try {
			localStorage.setItem('sumidoku.reader.dir', d);
		} catch {
			// 同上
		}
	}

	function scrollToPage(i: number) {
		contEl
			?.querySelector(`[data-page="${i}"]`)
			?.scrollIntoView({ block: 'start', behavior: 'instant' });
	}

	// 起始页：?page > 章节阅读进度 > 0；章节导航数据没就位前不动，免得按错的进度定位
	const navReady = $derived(!navEnabled || chaptersQuery.isFetched);
	$effect(() => {
		if (!pages || !navReady || inited) return;
		inited = true;
		const resume = untrack(() => rawChapters.find((c) => c.id === chapterId)?.lastPageRead ?? 0);
		const initial = Math.min(
			Math.max(startPage ?? (resume > 0 ? resume : 0), 0),
			Math.max(pages.length - 1, 0)
		);
		idx = initial;
		furthest = initial;
		col = 0;
		cur = initial;
		prefetch(initial);
		if (untrack(() => mode) !== 'paged') scrollToPage(initial);
	});

	// 切到滚动/条漫后定位回当前页
	let prevMode: Mode = untrack(() => mode);
	$effect(() => {
		const m = mode;
		if (prevMode === m) return;
		prevMode = m;
		untrack(() => {
			if (m === 'paged') {
				// 回到翻页时对齐到滚动模式当前页，而不是离开时停留的 idx
				idx = cur;
				col = 0;
			} else {
				// 进入滚动/条漫时，页码回到翻页模式当前页，而不是沿用历史最远页
				cur = idx;
				if (inited) scrollToPage(idx);
			}
		});
	});

	// 翻页模式下把文本页按视口切成等宽多栏；实测栏数用于分页和小节跳转
	$effect(() => {
		if (mode !== 'paged') return;
		// 依赖 idx/pages，与 React 的 [mode, idx, pages] 对齐
		idx;
		pages;
		const el = textColsEl;
		if (!el) return;
		const recompute = () => {
			const w = el.clientWidth;
			if (w <= 0) return;
			el.style.columnWidth = `${w}px`;
			const gap = parseFloat(getComputedStyle(el).columnGap) || 0;
			const cols = Math.max(1, Math.round((el.scrollWidth + gap) / (w + gap)));
			const prev = untrack(() => textLayout);
			if (!(prev.w === w && prev.gap === gap && prev.cols === cols)) {
				textLayout = { w, gap, cols };
			}
			if (untrack(() => col) > cols - 1) col = cols - 1;
		};
		recompute();
		const ro = new ResizeObserver(recompute);
		ro.observe(el);
		return () => ro.disconnect();
	});

	// 滚动/条漫模式进度 = 进入过视口的最大页码
	$effect(() => {
		if (mode === 'paged' || !pages || !navReady) return;
		const root = contEl;
		if (!root) return;
		const els = root.querySelectorAll('[data-page]');
		const ob = new IntersectionObserver((es) => {
			for (const e of es) {
				if (!e.isIntersecting) continue;
				const i = Number((e.target as HTMLElement).dataset.page);
				if (i > furthest) {
					furthest = i;
					scheduleProgress(i, i >= (pages?.length ?? 0) - 1);
				}
			}
		});
		els.forEach((el) => ob.observe(el));
		return () => ob.disconnect();
	});

	// 滚动/条漫模式页码 = 覆盖视口顶部的那一页，随滚动实时更新
	$effect(() => {
		if (mode === 'paged' || !pages || !navReady) return;
		const root = contEl;
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
			if (best !== null && best.i !== cur) cur = best.i;
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
	});

	// 轻点中部呼出工具栏，滑动不触发
	let down = { x: 0, y: 0 };
	function onPointerDown(e: PointerEvent) {
		down = { x: e.clientX, y: e.clientY };
	}
	function onPointerUp(e: PointerEvent) {
		if (Math.hypot(e.clientX - down.x, e.clientY - down.y) > 10) return;
		const third = window.innerWidth / 3;
		if (e.clientX > third && e.clientX < third * 2) overlay = !overlay;
	}

	function onSlider(e: Event) {
		const i = Number((e.currentTarget as HTMLInputElement).value) - 1;
		if (mode === 'paged') {
			if (textOnly) {
				const cols = textLayout.cols;
				const c = Math.min(Math.max(i, 0), Math.max(cols - 1, 0));
				col = c;
				scheduleProgress(idx, c >= cols - 1);
			} else {
				gotoPage(i);
			}
			return;
		}
		scrollToPage(i);
		idx = i;
		cur = i;
		if (i > furthest) {
			furthest = i;
			scheduleProgress(i, i >= total - 1);
		}
	}
</script>

{#snippet modeLabel(Icon: typeof BookOpen, label: string)}
	<span class="reader__mode" title={label}>
		<Icon size={15} />
		<span class="reader__mode-label">{label}</span>
	</span>
{/snippet}

{#snippet modePaged()}{@render modeLabel(BookOpen, $_('reader.modePaged'))}{/snippet}
{#snippet modeContinuous()}{@render modeLabel(AlignJustify, $_('reader.modeContinuous'))}{/snippet}
{#snippet modeWebtoon()}{@render modeLabel(StretchVertical, $_('reader.modeWebtoon'))}{/snippet}

{#if pagesQuery.isPending}
	<div class="reader reader--msg"><Spinner /></div>
{:else if pagesQuery.isError}
	<div class="reader reader--msg">
		<EmptyState icon={TriangleAlert} title={$_('reader.loadFailed')} hint={pagesQuery.error?.message}>
			{#snippet action()}
				<Button variant="primary" onclick={() => void pagesQuery.refetch()}>
					{$_('common.retry')}
				</Button>
			{/snippet}
		</EmptyState>
	</div>
{:else if !pages || pages.length === 0}
	<div class="reader reader--msg">
		<EmptyState icon={BookOpen} title={$_('reader.noPages')}>
			{#snippet action()}
				<Button onclick={goBack}>{$_('reader.back')}</Button>
			{/snippet}
		</EmptyState>
	</div>
{:else}
	<div class="reader">
		{#if mode === 'paged'}
			<div class="reader__paged">
				{#if curPage}
					{#if curText !== null}
						<div class="reader__text-paged">
							<!-- 沿用旧版信任模型，markdown 结果不做 sanitize -->
							<div
								class="reader-text reader-text--cols"
								bind:this={textColsEl}
								style={textColsStyle}
							>
								{@html md(curText)}
							</div>
						</div>
					{:else if isUnsupported(curPage)}
						<div class="reader__unsupported">{$_('reader.unsupportedPage')}</div>
					{:else}
						<img
							class="reader__img"
							src={pageUrl(chapterId, curPage.index, imgWidth)}
							alt={$_('reader.pageAlt', { values: { n: idx + 1 } })}
						/>
					{/if}
				{/if}
				<button
					type="button"
					class="reader__zone reader__zone--left"
					aria-label={$_('reader.prevPage')}
					onclick={() => onZone('left')}
				></button>
				<button
					type="button"
					class="reader__zone reader__zone--right"
					aria-label={$_('reader.nextPage')}
					onclick={() => onZone('right')}
				></button>
				<button
					type="button"
					class="reader__zone reader__zone--mid"
					aria-label={$_('reader.toggleToolbar')}
					onclick={() => (overlay = !overlay)}
				></button>
			</div>
		{:else}
			<div
				class={mode === 'webtoon' ? 'reader__cont reader__cont--webtoon' : 'reader__cont'}
				bind:this={contEl}
				onpointerdown={onPointerDown}
				onpointerup={onPointerUp}
			>
				{#each pages ?? [] as p (p.index)}
					{@const text = pageText(p)}
					<div class="reader__pg" data-page={p.index}>
						{#if text !== null}
							<!-- 沿用旧版信任模型，markdown 结果不做 sanitize -->
							<div class="reader-text">{@html md(text)}</div>
						{:else if isUnsupported(p)}
							<div class="reader__unsupported">{$_('reader.unsupportedPage')}</div>
						{:else}
							<img
								src={pageUrl(chapterId, p.index, imgWidth)}
								alt={$_('reader.pageAlt', { values: { n: p.index + 1 } })}
								loading="lazy"
							/>
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		<div class={['reader__bar', 'reader__bar--top', !overlay && 'reader__bar--hidden']}>
			<IconButton label={$_('reader.back')} onclick={goBack}>
				<ArrowLeft size={18} />
			</IconButton>
			<div class="reader__titles">
				<div class="reader__title">{mangaTitle || $_('reader.reading')}</div>
				{#if curChapter}<div class="reader__subtitle">{chapterLabel(curChapter)}</div>{/if}
			</div>
			<SegmentedControl
				options={[
					{ value: 'paged', label: modePaged },
					{ value: 'continuous', label: modeContinuous },
					{ value: 'webtoon', label: modeWebtoon }
				]}
				value={mode}
				onChange={setMode}
			/>
			<IconButton
				label={$_('reader.settings')}
				class={settingsOpen ? 'reader__icon--on' : undefined}
				onclick={() => (settingsOpen = !settingsOpen)}
			>
				<Settings size={17} />
			</IconButton>
		</div>

		{#if overlay && settingsOpen}
			<div class="reader__settings card">
				<div class="reader__settings-title">
					{mode !== 'paged'
						? $_('reader.readingDirectionPagedOnly')
						: $_('reader.readingDirection')}
				</div>
				<div class={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
					<SegmentedControl
						options={[
							{ value: 'ltr', label: $_('reader.dirLtr') },
							{ value: 'rtl', label: $_('reader.dirRtl') }
						]}
						value={dir}
						onChange={setDir}
					/>
				</div>
			</div>
		{/if}

		<div class={['reader__bar', 'reader__bar--bottom', !overlay && 'reader__bar--hidden']}>
			<Button size="sm" disabled={!prevCh} onclick={() => navCh(prevCh)}>
				{$_('reader.prevChapter')}
			</Button>
			<div class="reader__pager">
				<input
					type="range"
					min={1}
					max={navTotal}
					value={Math.min(navPos + 1, navTotal)}
					oninput={onSlider}
					aria-label={$_('reader.jumpToPage')}
				/>
				<span class="reader__page-num">{Math.min(navPos + 1, navTotal)}/{navTotal}</span>
			</div>
			<Button size="sm" disabled={!nextCh} onclick={() => navCh(nextCh)}>
				{$_('reader.nextChapter')}
			</Button>
		</div>
	</div>
{/if}
