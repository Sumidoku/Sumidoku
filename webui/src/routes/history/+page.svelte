<script lang="ts">
	import { onMount } from 'svelte';
	import { api, coverUrl } from '#lib/api';
	import Cover from '#lib/components/Cover.svelte';
	import { chapterLabel, relativeTime } from '#lib/format';
	import type { HistoryPage } from '#lib/types';

	let entries = $state<HistoryPage['entries']>([]);
	let page = $state(1);
	let hasNext = $state(false);
	let loading = $state(false);
	let initial = $state(true);
	let sentinel = $state<HTMLDivElement | null>(null);

	async function loadPage(p: number) {
		loading = true;
		try {
			const data = await api<HistoryPage>(`/history?page=${p}`);
			entries = p === 1 ? data.entries : [...entries, ...data.entries];
			hasNext = data.hasNextPage;
			page = p;
		} finally {
			loading = false;
			initial = false;
		}
	}

	onMount(() => loadPage(1));

	$effect(() => {
		if (!sentinel) return;
		const ob = new IntersectionObserver((es) => {
			if (es[0].isIntersecting && hasNext && !loading) loadPage(page + 1);
		});
		ob.observe(sentinel);
		return () => ob.disconnect();
	});
</script>

<div class="page">
	<h1 style="margin-bottom: 14px; font-size: 20px;">阅读历史</h1>

	{#if initial}
		<div class="list">
			{#each Array(6) as _, i (i)}
				<div class="skeleton" style="height: 76px;"></div>
			{/each}
		</div>
	{:else if entries.length === 0}
		<div class="empty">还没有阅读记录</div>
	{:else}
		<div class="list">
			{#each entries as h (h.chapter.id)}
				<a class="item card" href="/reader/{h.chapter.id}?manga={h.manga.id}&page={h.lastPageRead}">
					<div class="thumb">
						<Cover src={coverUrl(h.manga.id)} title={h.manga.title} />
					</div>
					<div class="meta">
						<div class="m-title">{h.manga.title}</div>
						<div class="muted" style="font-size: 13.5px;">{chapterLabel(h.chapter)}</div>
						<div class="faint" style="font-size: 12.5px; margin-top: 2px;">
							{relativeTime(h.lastReadAt)} · {h.isRead ? '已读完' : `读到 P.${h.lastPageRead + 1}`}
						</div>
					</div>
				</a>
			{/each}
		</div>
		<div bind:this={sentinel} style="height: 1px;"></div>
		{#if loading}
			<div style="text-align: center; padding: 16px;"><span class="spin"></span></div>
		{/if}
	{/if}
</div>

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.item {
		display: flex;
		gap: 12px;
		align-items: center;
		transition: background 0.15s;
	}

	.item:hover {
		background: var(--card-hover);
	}

	.thumb {
		width: 48px;
		flex: none;
	}

	.thumb :global(.cover-img),
	.thumb :global(.cover-placeholder) {
		border-radius: 6px;
		font-size: 11px;
	}

	.meta {
		min-width: 0;
	}

	.m-title {
		font-weight: 600;
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
