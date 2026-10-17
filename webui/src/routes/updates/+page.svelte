<script lang="ts">
	import { onMount } from 'svelte';
	import { ApiError, api, coverUrl } from '#lib/api';
	import Cover from '#lib/components/Cover.svelte';
	import { chapterLabel, relativeTime } from '#lib/format';
	import { pushToast } from '#lib/toast.svelte';
	import { subscribeWS } from '#lib/ws';
	import type {
		Category,
		UpdateJob,
		UpdateJobStatus,
		UpdateRecentEntry,
		UpdateRecentPage,
		UpdateSummary
	} from '#lib/types';

	let summary = $state<UpdateSummary | null>(null);
	let summaryLoaded = $state(false);
	let online = $state(false);
	let updating = $state(false);
	let cats = $state<Category[]>([]);
	let catSel = $state('');

	interface FeedJob extends UpdateJob {
		seq: number;
	}
	let jobs = $state<FeedJob[]>([]);
	let jobSeq = 0;

	let recent = $state<UpdateRecentEntry[]>([]);
	let page = $state(1);
	let hasNext = $state(false);
	let recentLoading = $state(false);
	let initial = $state(true);
	let sentinel = $state<HTMLDivElement | null>(null);

	const JOB_LABEL: Record<UpdateJobStatus, string> = {
		pending: '等待',
		running: '更新中',
		complete: '完成',
		failed: '失败',
		skipped: '跳过'
	};

	function num(v: unknown): number | undefined {
		return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
	}

	function isSummary(v: unknown): v is UpdateSummary {
		return !!v && typeof v === 'object' && typeof (v as Record<string, unknown>).running === 'boolean';
	}

	function onMessage(msg: unknown) {
		if (!msg || typeof msg !== 'object' || Array.isArray(msg)) return;
		const m = msg as Record<string, unknown>;
		if (m.type === 'snapshot') {
			if (isSummary(m.summary)) summary = m.summary;
			return;
		}
		// 快照帧兼容：也可能直接是 summary 对象
		if (isSummary(msg)) {
			summary = msg;
			return;
		}
		if (m.type !== 'job') return;
		const job: UpdateJob = {
			status: (typeof m.status === 'string' ? m.status : 'pending') as UpdateJobStatus,
			mangaId: num(m.mangaId) ?? 0,
			mangaTitle: typeof m.mangaTitle === 'string' ? m.mangaTitle : '',
			sourceId: num(m.sourceId) ?? 0,
			addedChapters: num(m.addedChapters) ?? 0
		};
		jobs = [{ ...job, seq: ++jobSeq }, ...jobs].slice(0, 50);
		// 收到 job 事件说明有任务在跑；契约没有「整轮结束」事件，running 的落停靠下方轮询对账
		if (summary && !summary.running) summary = { ...summary, running: true };
	}

	async function runUpdate(categoryId?: number) {
		if (updating) return;
		updating = true;
		try {
			await api('/update/fetch', {
				method: 'POST',
				body: categoryId !== undefined ? { categoryId } : {},
				silent: true
			});
			pushToast('已开始更新', 'info');
			if (summary) summary = { ...summary, running: true };
		} catch (e) {
			if (e instanceof ApiError && e.status === 409) pushToast('更新已在进行中', 'info');
			else pushToast(e instanceof Error ? e.message : '更新失败');
		} finally {
			updating = false;
		}
	}

	async function loadRecent(p: number) {
		recentLoading = true;
		try {
			const data = await api<UpdateRecentPage>(`/update/recent?page=${p}`);
			recent = p === 1 ? data.entries : [...recent, ...data.entries];
			hasNext = data.hasNextPage;
			page = p;
		} finally {
			recentLoading = false;
			initial = false;
		}
	}

	// 运行中每 4s 拉一次 summary 对账：计数刷新 + running 落停（WS 不推整轮结束事件）
	$effect(() => {
		if (!summary?.running) return;
		const t = setInterval(async () => {
			try {
				summary = await api<UpdateSummary>('/update/summary', { silent: true });
			} catch {
				// 失败等下一轮
			}
		}, 4000);
		return () => clearInterval(t);
	});

	$effect(() => {
		if (!sentinel) return;
		const ob = new IntersectionObserver((es) => {
			if (es[0].isIntersecting && hasNext && !recentLoading) loadRecent(page + 1);
		});
		ob.observe(sentinel);
		return () => ob.disconnect();
	});

	onMount(() => {
		const unsub = subscribeWS('/api/v1/update/ws', {
			onMessage,
			onStatus: (v) => (online = v)
		});
		void (async () => {
			try {
				summary = await api<UpdateSummary>('/update/summary');
			} finally {
				summaryLoaded = true;
			}
		})();
		void (async () => {
			try {
				cats = await api<Category[]>('/categories');
			} catch {
				// 分类只是可选过滤，拉不到就不显示下拉
			}
		})();
		void loadRecent(1);
		return unsub;
	});
</script>

<svelte:head>
	<title>更新 - Suzuri</title>
</svelte:head>

<div class="page">
	<div class="bar">
		<h1 style="font-size: 20px;">更新</h1>
		<span class="dot" class:on={online} title={online ? '实时连接正常' : '实时连接断开'}></span>
	</div>

	{#if !summaryLoaded}
		<div class="skeleton" style="height: 150px;"></div>
	{:else if !summary}
		<div class="card muted">更新摘要不可用</div>
	{:else}
		<div class="card summary">
			<div class="sum-head">
				{#if summary.running}
					<span class="spin"></span>
					<span class="run-text">更新进行中</span>
				{:else}
					<span class="run-text idle">空闲</span>
				{/if}
				{#if summary.lastRunAt}
					<span class="faint last">上次更新 {relativeTime(summary.lastRunAt)}</span>
				{/if}
			</div>
			<div class="counts">
				<div class="count">
					<div class="cv">{summary.total}</div>
					<div class="ck">总数</div>
				</div>
				<div class="count ok">
					<div class="cv">{summary.complete}</div>
					<div class="ck">完成</div>
				</div>
				<div class="count bad">
					<div class="cv">{summary.failed}</div>
					<div class="ck">失败</div>
				</div>
				<div class="count">
					<div class="cv">{summary.skipped}</div>
					<div class="ck">跳过</div>
				</div>
				<div class="count">
					<div class="cv">{summary.pending}</div>
					<div class="ck">待处理</div>
				</div>
			</div>
			<div class="run-ops">
				<button class="primary" disabled={updating} onclick={() => runUpdate()}>立即更新</button>
				{#if cats.length > 0}
					<div class="cat-op">
						<select bind:value={catSel}>
							<option value="">选择分类…</option>
							{#each cats as c (c.id)}
								<option value={String(c.id)}>{c.name}</option>
							{/each}
						</select>
						<button disabled={updating || catSel === ''} onclick={() => runUpdate(Number(catSel))}>
							按分类更新
						</button>
					</div>
				{/if}
			</div>
		</div>
	{/if}

	{#if jobs.length > 0}
		<div class="jobs card">
			<div class="faint" style="font-size: 12.5px; margin-bottom: 8px;">实时动态</div>
			{#each jobs as j (j.seq)}
				<div class="job">
					<span class="jdot {j.status}"></span>
					<span class="jt">{j.mangaTitle}</span>
					{#if j.addedChapters > 0}
						<span class="faint ja">+{j.addedChapters} 章</span>
					{/if}
					<span class="js {j.status}">{JOB_LABEL[j.status] ?? j.status}</span>
				</div>
			{/each}
		</div>
	{/if}

	<h2 style="margin: 18px 0 10px; font-size: 17px;">最近更新</h2>
	{#if initial}
		<div class="list">
			{#each Array(4) as _, i (i)}
				<div class="skeleton" style="height: 76px;"></div>
			{/each}
		</div>
	{:else if recent.length === 0}
		<div class="empty">最近没有新章节</div>
	{:else}
		<div class="list">
			{#each recent as e (e.chapter.id)}
				<a class="item card" href="/manga/{e.manga.id}">
					<div class="thumb">
						<Cover src={coverUrl(e.manga.id)} title={e.manga.title} />
					</div>
					<div class="meta">
						<div class="m-title">{e.manga.title}</div>
						<div class="muted" style="font-size: 13.5px;">
							{chapterLabel(e.chapter)}
							<span class="new-badge">新章</span>
						</div>
						<div class="faint" style="font-size: 12.5px; margin-top: 2px;">
							{relativeTime(e.fetchedAt ?? e.fetched_at)}
						</div>
					</div>
				</a>
			{/each}
		</div>
		<div bind:this={sentinel} style="height: 1px;"></div>
		{#if recentLoading}
			<div style="text-align: center; padding: 16px;"><span class="spin"></span></div>
		{/if}
	{/if}
</div>

<style>
	.bar {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 14px;
	}

	.dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--text-faint);
		flex: none;
	}

	.dot.on {
		background: var(--ok);
		box-shadow: 0 0 6px color-mix(in srgb, var(--ok) 70%, transparent);
	}

	.sum-head {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.run-text {
		font-weight: 600;
	}

	.run-text.idle {
		color: var(--text-dim);
	}

	.last {
		margin-left: auto;
		font-size: 12.5px;
	}

	.counts {
		display: flex;
		gap: 18px;
		margin-top: 12px;
		flex-wrap: wrap;
	}

	.cv {
		font-size: 20px;
		font-weight: 700;
	}

	.count.ok .cv {
		color: var(--ok);
	}

	.count.bad .cv {
		color: var(--danger);
	}

	.ck {
		font-size: 12px;
		color: var(--text-faint);
	}

	.run-ops {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-top: 14px;
		flex-wrap: wrap;
	}

	.cat-op {
		display: flex;
		gap: 8px;
	}

	.jobs {
		margin-top: 12px;
		max-height: 260px;
		overflow-y: auto;
	}

	.job {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 0;
		font-size: 13.5px;
	}

	.jdot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--text-faint);
		flex: none;
	}

	.jdot.running {
		background: var(--accent);
	}

	.jdot.complete {
		background: var(--ok);
	}

	.jdot.failed {
		background: var(--danger);
	}

	.jt {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.ja {
		font-size: 12.5px;
		flex: none;
	}

	.js {
		font-size: 12.5px;
		flex: none;
		color: var(--text-dim);
	}

	.js.running {
		color: var(--accent);
	}

	.js.complete {
		color: var(--ok);
	}

	.js.failed {
		color: var(--danger);
	}

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

	.new-badge {
		display: inline-block;
		padding: 1px 7px;
		border-radius: 999px;
		font-size: 11.5px;
		color: var(--accent);
		border: 1px solid var(--accent-dim);
		background: color-mix(in srgb, var(--accent) 14%, var(--bg-raised));
		vertical-align: 1px;
	}
</style>
