<script lang="ts">
	import { onMount } from 'svelte';
	import { api, coverUrl } from '#lib/api';
	import Cover from '#lib/components/Cover.svelte';
	import type { Category, LibraryPage } from '#lib/types';

	type Tab = { key: string; label: string; categoryId?: number };

	let categories = $state<Category[]>([]);
	let tabs = $derived<Tab[]>([
		{ key: 'all', label: '全部' },
		{ key: '0', label: '未分类', categoryId: 0 },
		...categories.map((c) => ({
			key: String(c.id),
			label: `${c.name}（${c.mangaCount}）`,
			categoryId: c.id
		}))
	]);

	let active = $state('all');
	let entries = $state<LibraryPage['entries']>([]);
	let page = $state(1);
	let hasNext = $state(false);
	let loading = $state(false);
	let initial = $state(true);
	let sentinel = $state<HTMLDivElement | null>(null);

	async function loadPage(p: number, categoryId: number | undefined) {
		loading = true;
		try {
			const qs = new URLSearchParams({ page: String(p) });
			if (categoryId !== undefined) qs.set('categoryId', String(categoryId));
			const data = await api<LibraryPage>(`/library?${qs}`);
			entries = p === 1 ? data.entries : [...entries, ...data.entries];
			hasNext = data.hasNextPage;
			page = p;
		} finally {
			loading = false;
			initial = false;
		}
	}

	function switchTab(key: string) {
		if (key === active) return;
		active = key;
		entries = [];
		const tab = tabs.find((t) => t.key === key);
		loadPage(1, tab?.categoryId);
	}

	onMount(async () => {
		try {
			categories = await api<Category[]>('/categories');
		} catch {
			// 分类加载失败不阻塞书库列表
		}
		await loadPage(1, undefined);
	});

	$effect(() => {
		if (!sentinel) return;
		const ob = new IntersectionObserver((es) => {
			if (es[0].isIntersecting && hasNext && !loading) {
				const tab = tabs.find((t) => t.key === active);
				loadPage(page + 1, tab?.categoryId);
			}
		});
		ob.observe(sentinel);
		return () => ob.disconnect();
	});
</script>

<div class="page">
	{#if tabs.length > 1}
		<div class="tabs" style="margin-bottom: 14px;">
			{#each tabs as tab (tab.key)}
				<button class="tab" class:active={tab.key === active} onclick={() => switchTab(tab.key)}>
					{tab.label}
				</button>
			{/each}
		</div>
	{/if}

	{#if initial}
		<div class="manga-grid">
			{#each Array(10) as _, i (i)}
				<div>
					<div class="skeleton" style="aspect-ratio: 3/4;"></div>
					<div class="skeleton" style="height: 14px; margin-top: 8px;"></div>
				</div>
			{/each}
		</div>
	{:else if entries.length === 0}
		<div class="empty">
			<p>书库还是空的</p>
			<p class="faint">去「浏览」里挑几本漫画加入书库吧</p>
		</div>
	{:else}
		<div class="manga-grid">
			{#each entries as m (m.id)}
				<a class="manga-card" href="/manga/{m.id}">
					<Cover src={coverUrl(m.id)} title={m.title} />
					<div class="title">{m.title}</div>
				</a>
			{/each}
		</div>
		<div bind:this={sentinel} style="height: 1px;"></div>
		{#if loading}
			<div style="text-align: center; padding: 16px;"><span class="spin"></span></div>
		{/if}
	{/if}
</div>
