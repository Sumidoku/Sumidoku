<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { onMount, onDestroy } from 'svelte';
	import { ApiError, api, coverUrl } from '$lib/api/client';
	import { useCategories, useFetchUpdate, useUpdateRecent } from '$lib/api/hooks';
	import type { UpdateJob, UpdateJobStatus, UpdateSummary } from '$lib/api/types';
	import type { EventSnapshot, UpdateEvent } from '$lib/api/protocol';
	import { sse } from '$lib/api/sse';
	import { chapterLabel, relativeTime } from '$lib/format';
	import {
		Badge,
		Button,
		Cover,
		InfiniteList,
		PageHeader,
		Select,
		Skeleton,
		Spinner,
		pushToast
	} from '$lib/components';

	// job 状态文案走 i18n key（updates 命名空间下的扁平 leaf）
	const JOB_LABEL: Record<UpdateJobStatus, string> = {
		pending: 'updates.jobPending',
		running: 'updates.jobRunning',
		complete: 'updates.jobComplete',
		failed: 'updates.jobFailed',
		skipped: 'updates.jobSkipped'
	};

	interface FeedJob extends UpdateJob {
		seq: number;
	}

	let summary = $state<UpdateSummary | null>(null);
	let summaryLoaded = $state(false);
	let online = $state(false);
	let updating = $state(false);
	let catSel = $state('');
	let jobs = $state<FeedJob[]>([]);
	let jobSeq = 0;

	const categories = useCategories();
	const fetchUpdate = useFetchUpdate();
	const recent = useUpdateRecent();
	const recentEntries = $derived(recent.data?.pages.flatMap((p) => p.entries) ?? []);

	let unsub: Array<() => void> = [];
	onMount(() => {
		let receivedEvent = false;

		// snapshot 的 updateSummary 字段即整份摘要
		function onSnapshot(msg: unknown) {
			const m = msg as EventSnapshot;
			receivedEvent = true;
			summary = m.updateSummary;
			summaryLoaded = true;
		}

		// updateSummary / updateComplete 帧的 payload 就是整份摘要（无 type 字段）
		function onSummary(msg: unknown) {
			receivedEvent = true;
			summary = msg as UpdateSummary;
			summaryLoaded = true;
		}

		// 整轮结束：running 落停由该事件拥有，不再靠轮询对账
		function onComplete(msg: unknown) {
			receivedEvent = true;
			summary = { ...(msg as UpdateSummary), running: false };
			summaryLoaded = true;
		}

		function onJob(msg: unknown) {
			const { type: _t, ...job } = msg as UpdateEvent;
			// 同一本漫画的连续事件只留最新一条，动态列表才不被刷屏
			jobs = [{ ...job, seq: ++jobSeq }, ...jobs.filter((j) => j.mangaId !== job.mangaId)].slice(
				0,
				50
			);
		}

		unsub = [
			sse.onStatus((o) => (online = o)),
			sse.on('snapshot', onSnapshot),
			sse.on('updateSummary', onSummary),
			sse.on('updateComplete', onComplete),
			sse.on('update', onJob)
		];
		// snapshot 一般随连接即到；SSE 慢或不可用时用一次整表拉作兜底，已收到事件则不覆盖
		void (async () => {
			try {
				const initial = await api<UpdateSummary>('/update/summary');
				if (!receivedEvent) summary = initial;
			} finally {
				summaryLoaded = true;
			}
		})();
	});
	onDestroy(() => unsub.forEach((u) => u()));

	const running = $derived(summary?.running === true);

	async function runUpdate(categoryId?: number) {
		if (updating) return;
		updating = true;
		try {
			await fetchUpdate.mutateAsync(categoryId);
			pushToast($_('updates.started'), 'info');
		} catch (e) {
			if (e instanceof ApiError && e.status === 409) pushToast($_('updates.busy'), 'info');
			else pushToast(e instanceof Error ? e.message : $_('updates.failed'));
		} finally {
			updating = false;
		}
	}

	const cats = $derived(categories.data ?? []);
</script>

<div class="page container">
	<PageHeader title={$_('nav.updates')} />

	{#if !summaryLoaded}
		<Skeleton height={150} />
	{:else if !summary}
		<div class="card muted">{$_('updates.summaryUnavailable')}</div>
	{:else}
		<div class="card updates-summary">
			<div class="updates-summary__head">
				{#if summary.running}
					<Spinner />
					<span class="updates-summary__status">{$_('updates.updating')}</span>
				{:else}
					<span class="updates-summary__status updates-summary__status--idle">
						{summary.lastRunAt
							? $_('updates.lastRun', { values: { time: relativeTime(summary.lastRunAt) } })
							: $_('updates.idle')}
					</span>
				{/if}
			</div>
			<div class="updates-summary__counts">
				<div class="updates-stat">
					<div class="updates-stat__v">{summary.total}</div>
					<div class="updates-stat__k">{$_('updates.statTotal')}</div>
				</div>
				<div class="updates-stat updates-stat--ok">
					<div class="updates-stat__v">{summary.complete}</div>
					<div class="updates-stat__k">{$_('updates.statComplete')}</div>
				</div>
				<div class="updates-stat updates-stat--bad">
					<div class="updates-stat__v">{summary.failed}</div>
					<div class="updates-stat__k">{$_('updates.statFailed')}</div>
				</div>
				<div class="updates-stat">
					<div class="updates-stat__v">{summary.skipped}</div>
					<div class="updates-stat__k">{$_('updates.statSkipped')}</div>
				</div>
				<div class="updates-stat">
					<div class="updates-stat__v">{summary.pending}</div>
					<div class="updates-stat__k">{$_('updates.statPending')}</div>
				</div>
			</div>
			<div class="updates-summary__ops">
				<span
					class={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
					title={online ? $_('updates.wsOnline') : $_('updates.wsOffline')}
				></span>
				<Button
					variant="primary"
					loading={updating}
					disabled={running}
					onclick={() => void runUpdate()}
				>
					{$_('updates.updateNow')}
				</Button>
				{#if cats.length > 0}
					<Select aria-label={$_('updates.selectCategory')} bind:value={catSel}>
						<option value="">{$_('updates.allCategories')}</option>
						{#each cats as c (c.id)}
							<option value={String(c.id)}>{c.name}</option>
						{/each}
					</Select>
					<Button
						disabled={updating || running || catSel === ''}
						onclick={() => void runUpdate(Number(catSel))}
					>
						{$_('updates.updateByCategory')}
					</Button>
				{/if}
			</div>
		</div>
	{/if}

	{#if jobs.length > 0}
		<div class="card updates-jobs">
			<div class="faint updates-jobs__title">{$_('updates.liveFeed')}</div>
			{#each jobs as j (j.seq)}
				<div class="updates-job">
					<span class="updates-job__dot updates-job__dot--{j.status}"></span>
					<span class="updates-job__title">{j.mangaTitle}</span>
					<span class="updates-job__status updates-job__status--{j.status}">
						{JOB_LABEL[j.status] ? $_(JOB_LABEL[j.status]) : j.status}
					</span>
					{#if j.addedChapters > 0}
						<span class="updates-job__added">
							{$_('updates.addedChapters', { values: { n: j.addedChapters } })}
						</span>
					{/if}
				</div>
			{/each}
		</div>
	{/if}

	<h2 class="updates-recent__heading">{$_('updates.recentHeading')}</h2>
	{#if recent.isPending}
		<div class="updates-list">
			{#each Array.from({ length: 4 }) as _item, i (i)}
				<Skeleton height={76} />
			{/each}
		</div>
	{:else if recentEntries.length === 0}
		<div class="updates-empty faint">{$_('updates.recentEmpty')}</div>
	{:else}
		<InfiniteList
			hasNextPage={recent.hasNextPage}
			isFetching={recent.isFetching}
			onLoadMore={() => void recent.fetchNextPage()}
		>
			<div class="updates-list">
				{#each recentEntries as e (e.chapter.id)}
					<a class="updates-item card" href={`/manga/${e.manga.id}`}>
						<div class="updates-item__thumb">
							<Cover src={coverUrl(e.manga.id, e.manga.cover)} title={e.manga.title} />
						</div>
						<div class="updates-item__meta">
							<div class="updates-item__title">{e.manga.title}</div>
							<div class="muted updates-item__chapter">
								{chapterLabel(e.chapter)}
								<Badge>{$_('updates.newChapter')}</Badge>
							</div>
							<div class="faint updates-item__time">
								{relativeTime(e.fetchedAt ?? e.fetched_at)}
							</div>
						</div>
					</a>
				{/each}
			</div>
		</InfiniteList>
	{/if}
</div>

<style>
	.ws-dot {
		width: 10px;
		height: 10px;
		border-radius: 50%;
		background: var(--text-faint);
		flex: none;
	}

	.ws-dot--on {
		background: var(--success);
		box-shadow: 0 0 6px color-mix(in srgb, var(--success) 70%, transparent);
	}

	.updates-summary__head {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.updates-summary__status {
		font-weight: 600;
	}

	.updates-summary__status--idle {
		color: var(--text-dim);
		font-weight: 600;
	}

	.updates-summary__counts {
		display: flex;
		gap: 18px;
		margin-top: 12px;
		flex-wrap: wrap;
	}

	.updates-stat__v {
		font-size: 20px;
		font-weight: 700;
	}

	.updates-stat--ok .updates-stat__v {
		color: var(--success);
	}

	.updates-stat--bad .updates-stat__v {
		color: var(--danger);
	}

	.updates-stat__k {
		font-size: 12px;
		color: var(--text-faint);
	}

	.updates-summary__ops {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-top: 14px;
		flex-wrap: wrap;
	}

	.updates-jobs {
		margin-top: 12px;
		max-height: 260px;
		overflow-y: auto;
	}

	.updates-jobs__title {
		font-size: 12.5px;
		margin-bottom: 8px;
	}

	.updates-job {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 0;
		font-size: 13.5px;
	}

	.updates-job__dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--text-faint);
		flex: none;
	}

	/* 变体类名由 j.status 动态拼接，Svelte 静态裁剪看不到引用，用 :global 防止被剪掉 */
	:global(.updates-job__dot--running) {
		background: var(--accent);
	}

	:global(.updates-job__dot--complete) {
		background: var(--success);
	}

	:global(.updates-job__dot--failed) {
		background: var(--danger);
	}

	:global(.updates-job__dot--skipped) {
		background: var(--warning);
	}

	.updates-job__title {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.updates-job__status {
		font-size: 12.5px;
		flex: none;
		color: var(--text-dim);
	}

	:global(.updates-job__status--running) {
		color: var(--accent);
	}

	:global(.updates-job__status--complete) {
		color: var(--success);
	}

	:global(.updates-job__status--failed) {
		color: var(--danger);
	}

	:global(.updates-job__status--skipped) {
		color: var(--warning);
	}

	.updates-job__added {
		font-size: 12.5px;
		flex: none;
		color: var(--accent);
	}

	.updates-recent__heading {
		margin: 18px 0 10px;
		font-size: 17px;
	}

	.updates-empty {
		padding: 24px 0;
		text-align: center;
	}

	.updates-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.updates-item {
		display: flex;
		align-items: center;
		gap: 12px;
		transition: background 0.15s;
	}

	.updates-item:hover {
		background: var(--bg-hover);
	}

	.updates-item__thumb {
		width: 56px;
		flex: none;
	}

	.updates-item__thumb :global(.cover),
	.updates-item__thumb :global(.cover__placeholder) {
		border-radius: var(--r-sm);
	}

	.updates-item__meta {
		flex: 1;
		min-width: 0;
	}

	.updates-item__title {
		font-weight: 600;
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.updates-item__chapter {
		font-size: 13.5px;
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.updates-item__time {
		font-size: 12.5px;
		margin-top: 2px;
	}
</style>
