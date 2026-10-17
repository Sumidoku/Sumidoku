<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { marked } from 'marked';
	import { api, pageUrl } from '#lib/api';
	import { chapterLabel } from '#lib/format';
	import type { Chapter, MangaDetail, PageData } from '#lib/types';

	let {
		chapterId,
		mangaId,
		startPage
	}: { chapterId: number; mangaId: number | null; startPage: number | null } = $props();

	type Mode = 'paged' | 'continuous' | 'webtoon';
	type Dir = 'ltr' | 'rtl';

	const MODES: { key: Mode; label: string }[] = [
		{ key: 'paged', label: '翻页' },
		{ key: 'continuous', label: '滚动' },
		{ key: 'webtoon', label: '条漫' }
	];

	let pages = $state<PageData[] | null>(null);
	let chapters = $state<Chapter[]>([]);
	let mangaTitle = $state('');
	let idx = $state(0);
	let furthest = $state(0);
	let overlay = $state(true);
	let settingsOpen = $state(false);
	let loading = $state(true);

	let mode = $state<Mode>('paged');
	let dir = $state<Dir>('ltr');

	let total = $derived(pages?.length ?? 0);
	let curPage = $derived(pages?.[idx]);
	let curChapter = $derived(chapters.find((c) => c.id === chapterId) ?? null);
	let pos = $derived(chapters.findIndex((c) => c.id === chapterId));
	let prevCh = $derived(pos > 0 ? chapters[pos - 1] : null);
	let nextCh = $derived(pos >= 0 && pos < chapters.length - 1 ? chapters[pos + 1] : null);
	let shown = $derived(mode === 'paged' ? idx : furthest);

	let contEl = $state<HTMLDivElement | null>(null);

	function isText(p: PageData | undefined | null): p is PageData & { content: { Text: string } } {
		return !!p?.content && typeof p.content === 'object' && 'Text' in p.content;
	}

	function md(text: string): string {
		return marked.parse(text, { async: false }) as string;
	}

	let timer: ReturnType<typeof setTimeout> | undefined;

	// 进度上报防抖 300ms；到达最后一页时顺带标记已读。失败静默，不打断阅读
	function scheduleProgress(i: number) {
		clearTimeout(timer);
		timer = setTimeout(() => {
			const last = total > 0 && i >= total - 1;
			const body: Record<string, unknown> = { lastPageRead: i };
			if (last) body.isRead = true;
			api(`/chapter/${chapterId}`, { method: 'PATCH', body, silent: true }).catch(() => {});
		}, 300);
	}

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
		if (clamped === idx && pages) return;
		idx = clamped;
		if (clamped > furthest) furthest = clamped;
		scheduleProgress(clamped);
		prefetch(clamped);
	}

	function onZone(which: 'left' | 'right') {
		const forward = dir === 'ltr' ? which === 'right' : which === 'left';
		gotoPage(idx + (forward ? 1 : -1));
	}

	async function setMode(m: Mode) {
		if (m === mode) return;
		mode = m;
		localStorage.setItem('suzuri.reader.mode', m);
		if (m !== 'paged') {
			await tick();
			scrollToPage(idx);
		}
	}

	function setDir(d: Dir) {
		dir = d;
		localStorage.setItem('suzuri.reader.dir', d);
	}

	function scrollToPage(i: number) {
		contEl
			?.querySelector(`[data-page="${i}"]`)
			?.scrollIntoView({ block: 'start', behavior: 'instant' });
	}

	function navCh(c: Chapter | null) {
		if (!c) return;
		goto(`/reader/${c.id}?manga=${mangaId}`);
	}

	function goBack() {
		if (mangaId !== null) goto(`/manga/${mangaId}`);
		else history.back();
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

	// 滚动/条漫模式：轻点中部呼出工具栏，滑动不触发
	let downX = 0;
	let downY = 0;
	function onPointerDown(e: PointerEvent) {
		downX = e.clientX;
		downY = e.clientY;
	}
	function onPointerUp(e: PointerEvent) {
		if (Math.hypot(e.clientX - downX, e.clientY - downY) > 10) return;
		const third = window.innerWidth / 3;
		if (e.clientX > third && e.clientX < third * 2) overlay = !overlay;
	}

	// 滚动模式进度 = 已入视口的最大页码
	$effect(() => {
		if (!contEl || !pages || mode === 'paged') return;
		const els = contEl.querySelectorAll('[data-page]');
		const ob = new IntersectionObserver((es) => {
			for (const e of es) {
				if (!e.isIntersecting) continue;
				const i = Number((e.target as HTMLElement).dataset.page);
				if (i > furthest) {
					furthest = i;
					scheduleProgress(i);
				}
			}
		});
		els.forEach((el) => ob.observe(el));
		return () => ob.disconnect();
	});

	onMount(() => {
		const m = localStorage.getItem('suzuri.reader.mode');
		if (m === 'paged' || m === 'continuous' || m === 'webtoon') mode = m;
		const d = localStorage.getItem('suzuri.reader.dir');
		if (d === 'ltr' || d === 'rtl') dir = d;

		window.addEventListener('keydown', onKey);

		void (async () => {
			try {
				const p = await api<PageData[]>(`/chapter/${chapterId}/pages`);
				pages = p;
				if (mangaId !== null) {
					try {
						const [md_, cs] = await Promise.all([
							api<MangaDetail>(`/manga/${mangaId}`),
							api<Chapter[]>(`/manga/${mangaId}/chapters`)
						]);
						mangaTitle = md_.title;
						chapters = cs;
					} catch {
						// 拿不到章节导航信息也能正常阅读本章
					}
				}
				const resume = chapters.find((c) => c.id === chapterId)?.lastPageRead ?? 0;
				const initial = Math.min(
					Math.max(startPage ?? (resume > 0 ? resume : 0), 0),
					Math.max(p.length - 1, 0)
				);
				idx = initial;
				furthest = initial;
				prefetch(initial);
				if (mode !== 'paged') {
					await tick();
					scrollToPage(initial);
				}
			} finally {
				loading = false;
			}
		})();

		return () => {
			window.removeEventListener('keydown', onKey);
			clearTimeout(timer);
		};
	});
</script>

<svelte:head>
	<title>{mangaTitle ? `${mangaTitle} · ` : ''}阅读 - Suzuri</title>
</svelte:head>

<div class="reader" class:webtoon={mode === 'webtoon'}>
	{#if loading}
		<div class="center-msg"><span class="spin"></span></div>
	{:else if !pages || pages.length === 0}
		<div class="center-msg muted">本章没有页面</div>
	{:else if mode === 'paged'}
		<div class="paged-view">
			{#if curPage}
				{#if isText(curPage)}
					<div class="text-scroll">
						<div class="md-body">{@html md(curPage.content.Text)}</div>
					</div>
				{:else}
					<img class="page-img" src={pageUrl(chapterId, curPage.index)} alt="第 {idx + 1} 页" />
				{/if}
			{/if}
			<button class="zone left" aria-label="上一页" onclick={() => onZone('left')}></button>
			<button class="zone right" aria-label="下一页" onclick={() => onZone('right')}></button>
			<button class="zone mid" aria-label="显示或隐藏工具栏" onclick={() => (overlay = !overlay)}>
			</button>
		</div>
	{:else}
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="cont-view"
			class:no-gap={mode === 'webtoon'}
			bind:this={contEl}
			onpointerdown={onPointerDown}
			onpointerup={onPointerUp}
		>
			{#each pages as p (p.index)}
				<div class="pg" data-page={p.index}>
					{#if isText(p)}
						<div class="md-body">{@html md(p.content.Text)}</div>
					{:else}
						<img src={pageUrl(chapterId, p.index)} alt="第 {p.index + 1} 页" loading="lazy" />
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	{#if overlay && !loading}
		<div class="bar top">
			<button class="icon" aria-label="返回" onclick={goBack}>←</button>
			<div class="titles">
				<div class="t1">{mangaTitle || '阅读'}</div>
				{#if curChapter}<div class="t2">{chapterLabel(curChapter)}</div>{/if}
			</div>
			<div class="modes">
				{#each MODES as m (m.key)}
					<button class:on={mode === m.key} onclick={() => setMode(m.key)}>{m.label}</button>
				{/each}
			</div>
			<button
				class="icon"
				aria-label="阅读设置"
				class:on={settingsOpen}
				onclick={() => (settingsOpen = !settingsOpen)}>⚙</button
			>
		</div>

		{#if settingsOpen}
			<div class="settings card">
				<div class="muted" style="font-size: 13px; margin-bottom: 8px;">
					阅读方向{mode !== 'paged' ? '（仅翻页模式生效）' : ''}
				</div>
				<div style="display: flex; gap: 8px;">
					<button disabled={mode !== 'paged'} class:on={dir === 'ltr'} onclick={() => setDir('ltr')}>
						从左到右
					</button>
					<button disabled={mode !== 'paged'} class:on={dir === 'rtl'} onclick={() => setDir('rtl')}>
						从右到左
					</button>
				</div>
			</div>
		{/if}

		<div class="bar bottom">
			<button disabled={!prevCh} onclick={() => navCh(prevCh)}>上一章</button>
			<span class="muted">{Math.min(shown + 1, total)} / {total}</span>
			<button disabled={!nextCh} onclick={() => navCh(nextCh)}>下一章</button>
		</div>
	{/if}
</div>

<style>
	.reader {
		min-height: 100dvh;
		background: #000;
	}

	.center-msg {
		min-height: 100dvh;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.paged-view {
		position: relative;
		height: 100dvh;
		display: flex;
		align-items: center;
		justify-content: center;
		overflow: hidden;
	}

	.page-img {
		max-width: 100%;
		max-height: 100%;
		object-fit: contain;
	}

	.text-scroll {
		height: 100%;
		overflow-y: auto;
		width: 100%;
		padding: 24px 16px;
	}

	.zone {
		position: absolute;
		top: 0;
		bottom: 0;
		border: none;
		background: transparent;
		border-radius: 0;
		padding: 0;
		cursor: pointer;
	}

	.zone.left {
		left: 0;
		width: 28%;
	}

	.zone.right {
		right: 0;
		width: 28%;
	}

	.zone.mid {
		left: 28%;
		width: 44%;
	}

	.cont-view {
		max-width: 780px;
		margin: 0 auto;
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 12px 0 40vh;
	}

	.cont-view.no-gap {
		max-width: none;
		gap: 0;
		padding: 0;
	}

	.pg img {
		width: 100%;
	}

	.cont-view:not(.no-gap) .pg {
		padding: 0 8px;
	}

	.bar {
		position: fixed;
		left: 0;
		right: 0;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 12px;
		background: color-mix(in srgb, var(--bg) 92%, transparent);
		backdrop-filter: blur(8px);
		z-index: 60;
	}

	.bar.top {
		top: 0;
		border-bottom: 1px solid var(--border);
	}

	.bar.bottom {
		bottom: 0;
		border-top: 1px solid var(--border);
		justify-content: space-between;
		padding-bottom: calc(8px + env(safe-area-inset-bottom));
	}

	.icon {
		border: none;
		background: none;
		font-size: 18px;
		padding: 6px 8px;
		flex: none;
	}

	.icon.on {
		color: var(--accent);
	}

	.titles {
		flex: 1;
		min-width: 0;
	}

	.t1 {
		font-size: 14px;
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.t2 {
		font-size: 12px;
		color: var(--text-dim);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.modes {
		display: flex;
		flex: none;
	}

	.modes button {
		border: 1px solid var(--border);
		background: var(--bg);
		border-radius: 0;
		padding: 5px 10px;
		font-size: 13px;
		color: var(--text-dim);
	}

	.modes button:first-child {
		border-radius: 8px 0 0 8px;
	}

	.modes button:last-child {
		border-radius: 0 8px 8px 0;
	}

	.modes button.on {
		background: var(--accent);
		border-color: var(--accent);
		color: #14161c;
		font-weight: 600;
	}

	button.on {
		background: color-mix(in srgb, var(--accent) 22%, var(--card));
		border-color: var(--accent-dim);
		color: var(--accent);
	}

	.settings {
		position: fixed;
		top: 56px;
		right: 12px;
		z-index: 61;
		width: min(300px, calc(100vw - 24px));
	}

	@media (max-width: 640px) {
		.t2 {
			display: none;
		}

		.modes button {
			padding: 5px 7px;
			font-size: 12px;
		}
	}
</style>
