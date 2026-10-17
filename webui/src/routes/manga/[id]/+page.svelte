<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api, coverUrl } from '#lib/api';
	import Cover from '#lib/components/Cover.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { pushToast } from '#lib/toast.svelte';
	import { chapterLabel, formatDate, statusText } from '#lib/format';
	import type { Category, Chapter, MangaDetail } from '#lib/types';

	const mangaId = Number(page.params.id);

	let manga = $state<MangaDetail | null>(null);
	let chapters = $state<Chapter[]>([]);
	let loading = $state(true);
	let descOpen = $state(false);
	let busy = $state(false);

	let catOpen = $state(false);
	let allCats = $state<Category[]>([]);
	let catSel = $state<Set<number>>(new Set());

	// 源返回序倒转：最新章节显示在最上
	let ordered = $derived([...chapters].reverse());

	async function toggleLibrary() {
		if (!manga || busy) return;
		busy = true;
		try {
			if (manga.inLibrary) {
				await api(`/library/${mangaId}`, { method: 'DELETE' });
				manga = { ...manga, inLibrary: false };
				pushToast('已移出书库', 'info');
			} else {
				await api('/library', { method: 'POST', body: { mangaId } });
				manga = { ...manga, inLibrary: true };
				pushToast('已加入书库', 'info');
			}
		} finally {
			busy = false;
		}
	}

	async function openCats() {
		catSel = new Set(manga?.categoryIds ?? []);
		catOpen = true;
		if (allCats.length === 0) {
			try {
				allCats = await api<Category[]>('/categories');
			} catch {
				catOpen = false;
			}
		}
	}

	async function saveCats() {
		busy = true;
		try {
			await api(`/manga/${mangaId}/categories`, {
				method: 'PUT',
				body: { categoryIds: [...catSel] }
			});
			if (manga) manga = { ...manga, categoryIds: [...catSel] };
			pushToast('分类已保存', 'info');
			catOpen = false;
		} finally {
			busy = false;
		}
	}

	function toggleCat(id: number) {
		const next = new Set(catSel);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		catSel = next;
	}

	async function toggleBookmark(c: Chapter) {
		const res = await api<{ isBookmarked: boolean }>(`/chapter/${c.id}`, {
			method: 'PATCH',
			body: { isBookmarked: !c.isBookmarked }
		});
		chapters = chapters.map((x) => (x.id === c.id ? { ...x, isBookmarked: res.isBookmarked } : x));
	}

	function readTarget(c: Chapter): string {
		const resume = !c.isRead && c.lastPageRead > 0 ? c.lastPageRead : 0;
		return `/reader/${c.id}?manga=${mangaId}&page=${resume}`;
	}

	onMount(async () => {
		try {
			const [m, cs] = await Promise.all([
				api<MangaDetail>(`/manga/${mangaId}`),
				api<Chapter[]>(`/manga/${mangaId}/chapters`)
			]);
			manga = m;
			chapters = cs;
		} finally {
			loading = false;
		}
	});
</script>

<div class="page">
	{#if loading}
		<div style="display: flex; gap: 16px;">
			<div class="skeleton" style="width: 140px; aspect-ratio: 3/4; flex: none;"></div>
			<div style="flex: 1;">
				<div class="skeleton" style="height: 24px; width: 60%;"></div>
				<div class="skeleton" style="height: 14px; width: 40%; margin-top: 10px;"></div>
				<div class="skeleton" style="height: 14px; width: 80%; margin-top: 10px;"></div>
			</div>
		</div>
	{:else if manga}
		<div class="head">
			<div class="cover">
				<Cover src={coverUrl(manga.id)} title={manga.title} />
			</div>
			<div class="info">
				<h1 style="font-size: 20px; line-height: 1.35;">{manga.title}</h1>
				{#if manga.authors.length > 0}
					<div class="muted" style="margin-top: 4px;">{manga.authors.join(' / ')}</div>
				{/if}
				<div style="margin-top: 6px;">
					<span class="chip">{statusText(manga.status)}</span>
				</div>
				{#if manga.tags.length > 0}
					<div class="tagrow">
						{#each manga.tags as t (t)}
							<span class="chip">{t}</span>
						{/each}
					</div>
				{/if}
				<div class="actions">
					{#if manga.inLibrary}
						<button class="danger" disabled={busy} onclick={toggleLibrary}>移出书库</button>
						<button disabled={busy} onclick={openCats}>分类</button>
					{:else}
						<button class="primary" disabled={busy} onclick={toggleLibrary}>加入书库</button>
					{/if}
				</div>
			</div>
		</div>

		{#if manga.description}
			<div class="desc card">
				<p class:clamp={!descOpen}>{manga.description}</p>
				<button class="link" onclick={() => (descOpen = !descOpen)}>
					{descOpen ? '收起' : '展开'}
				</button>
			</div>
		{/if}

		<h2 style="margin: 18px 0 10px; font-size: 17px;">章节（{chapters.length}）</h2>
		{#if chapters.length === 0}
			<div class="empty">暂无章节</div>
		{:else}
			<div class="chaps">
				{#each ordered as c (c.id)}
					<div class="chap" class:read={c.isRead}>
						<a class="chap-main" href={readTarget(c)}>
							<div class="chap-title">
								{chapterLabel(c)}
								{#if c.locked}<span class="faint"> 🔒</span>{/if}
							</div>
							<div class="chap-meta faint">
								{formatDate(c.date_uploaded)}
								{#if !c.isRead && c.lastPageRead > 0}
									<span class="resume">续读 P.{c.lastPageRead + 1}</span>
								{:else if c.isRead}
									<span>已读</span>
								{/if}
							</div>
						</a>
						<button
							class="bm"
							class:on={c.isBookmarked}
							title={c.isBookmarked ? '取消书签' : '加书签'}
							onclick={() => toggleBookmark(c)}
						>
							{c.isBookmarked ? '★' : '☆'}
						</button>
					</div>
				{/each}
			</div>
		{/if}

		<Modal open={catOpen} title="设置分类" onclose={() => (catOpen = false)}>
			{#if allCats.length === 0}
				<p class="muted">还没有分类，可先在设置页创建。</p>
			{:else}
				<div class="catlist">
					{#each allCats as c (c.id)}
						<label class="catitem">
							<input
								type="checkbox"
								checked={catSel.has(c.id)}
								onchange={() => toggleCat(c.id)}
							/>
							<span>{c.name}</span>
						</label>
					{/each}
				</div>
				<div style="display: flex; justify-content: flex-end; margin-top: 12px;">
					<button class="primary" disabled={busy} onclick={saveCats}>保存</button>
				</div>
			{/if}
		</Modal>
	{:else}
		<div class="empty">漫画不存在</div>
	{/if}
</div>

<style>
	.head {
		display: flex;
		gap: 16px;
	}

	.cover {
		width: 140px;
		flex: none;
	}

	.info {
		flex: 1;
		min-width: 0;
	}

	.tagrow {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 8px;
	}

	.actions {
		display: flex;
		gap: 8px;
		margin-top: 14px;
	}

	.desc {
		margin-top: 16px;
		font-size: 14px;
		color: var(--text-dim);
	}

	.desc p {
		margin: 0;
		white-space: pre-wrap;
	}

	.desc p.clamp {
		display: -webkit-box;
		line-clamp: 3;
		-webkit-line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.link {
		border: none;
		background: none;
		color: var(--accent);
		padding: 4px 0 0;
		font-size: 13.5px;
	}

	.chaps {
		display: flex;
		flex-direction: column;
	}

	.chap {
		display: flex;
		align-items: center;
		border-bottom: 1px solid var(--border);
	}

	.chap.read .chap-title {
		color: var(--text-faint);
	}

	.chap-main {
		flex: 1;
		min-width: 0;
		padding: 11px 4px;
	}

	.chap-main:hover .chap-title {
		color: var(--accent);
	}

	.chap-title {
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chap-meta {
		font-size: 12.5px;
		margin-top: 2px;
		display: flex;
		gap: 10px;
	}

	.resume {
		color: var(--accent);
	}

	.bm {
		border: none;
		background: none;
		font-size: 18px;
		color: var(--text-faint);
		padding: 8px 10px;
	}

	.bm.on {
		color: var(--accent);
	}

	.catlist {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.catitem {
		display: flex;
		align-items: center;
		gap: 10px;
		cursor: pointer;
	}

	@media (max-width: 640px) {
		.cover {
			width: 108px;
		}
	}
</style>
