<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { onMount, onDestroy } from 'svelte';
	import Download from '@lucide/svelte/icons/download';
	import Pause from '@lucide/svelte/icons/pause';
	import Play from '@lucide/svelte/icons/play';
	import RotateCw from '@lucide/svelte/icons/rotate-cw';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { api, chapterFileUrl } from '$lib/api/client';
	import {
		useDeleteDownload,
		useDownloadsClear,
		useDownloadsStart,
		useDownloadsStop,
		useEnqueueDownloads
	} from '$lib/api/hooks';
	import type { DownloadItem, DownloadStatus } from '$lib/api/types';
	import type { DownloadChapterEvent, EventSnapshot } from '$lib/api/protocol';
	import { sse } from '$lib/api/sse';
	import { applyDownloadEvent, formatDownloadProgress, mergeDownloadResponse } from '$lib/downloadEvents';
	import { chapterLabel } from '$lib/format';
	import {
		Button,
		Chip,
		EmptyState,
		IconButton,
		PageHeader,
		ProgressBar,
		Skeleton,
		pushToast
	} from '$lib/components';

	type ChipColor = 'default' | 'accent' | 'danger' | 'success' | 'warning';

	const ORDER: DownloadStatus[] = ['queued', 'downloading', 'paused', 'error', 'finished'];

	// 状态标签 + chip 颜色；标签走 i18n key（downloads 命名空间下的扁平 leaf）
	const STATUS_META: Record<DownloadStatus, { key: string; color: ChipColor }> = {
		queued: { key: 'downloads.statusQueued', color: 'default' },
		downloading: { key: 'downloads.statusDownloading', color: 'accent' },
		paused: { key: 'downloads.statusPaused', color: 'warning' },
		finished: { key: 'downloads.statusFinished', color: 'success' },
		error: { key: 'downloads.statusError', color: 'danger' }
	};

	let items = $state<DownloadItem[]>([]);
	let running = $state(false);
	let online = $state(false);
	let loading = $state(true);
	let busy = $state(false);

	const start = useDownloadsStart();
	const stop = useDownloadsStop();
	const clear = useDownloadsClear();
	const del = useDeleteDownload();
	const enqueue = useEnqueueDownloads();

	// $state 代理在回调里读取即是最新值，无需 React 版的 itemsRef
	function applyItems(next: DownloadItem[]) {
		items = next;
	}

	// 事件只带 chapterId，遇到没见过的任务要整表拉一次补齐元数据；
	// 合并期间到达的事件改写最新值而不是被整表覆盖，snapshot 到来则作废本轮补拉
	let refetching = false;
	let refreshAgain = false;
	let snapshotVersion = 0;
	const pendingEvents = new Map<number, DownloadChapterEvent>();
	async function refreshMissing() {
		if (refetching) {
			refreshAgain = true;
			return;
		}
		refetching = true;
		try {
			do {
				refreshAgain = false;
				const version = snapshotVersion;
				const response = await api<DownloadItem[]>('/downloads');
				if (version === snapshotVersion) {
					applyItems(mergeDownloadResponse(items, response, pendingEvents));
					pendingEvents.clear();
				}
			} while (refreshAgain);
		} finally {
			refetching = false;
			refreshAgain = false;
			pendingEvents.clear();
		}
	}

	let unsub: Array<() => void> = [];
	onMount(() => {
		let receivedEvent = false;

		function onSnapshot(msg: unknown) {
			const m = msg as EventSnapshot;
			if (m.type !== 'snapshot') return;
			receivedEvent = true;
			snapshotVersion += 1;
			refreshAgain = false;
			pendingEvents.clear();
			applyItems(m.downloads);
			running = m.running;
			loading = false;
		}

		function onRunning(msg: unknown) {
			receivedEvent = true;
			running = (msg as { running: boolean }).running;
		}

		function onDownload(msg: unknown) {
			const m = msg as DownloadChapterEvent;
			receivedEvent = true;
			const missing = !items.some((it) => it.chapterId === m.chapterId);
			if (refetching || missing) pendingEvents.set(m.chapterId, m);
			if (refetching) refreshAgain = true;
			applyItems(applyDownloadEvent(items, m));
			if (missing) void refreshMissing();
		}

		unsub = [
			sse.onStatus((o) => (online = o)),
			sse.on('snapshot', onSnapshot),
			sse.on('running', onRunning),
			sse.on('download', onDownload)
		];
		// snapshot 一般随连接即到；SSE 慢或不可用时用一次整表拉作兜底，已收到事件则不覆盖
		void (async () => {
			try {
				const initial = await api<DownloadItem[]>('/downloads');
				if (!receivedEvent) applyItems(initial);
			} finally {
				loading = false;
			}
		})();
	});
	onDestroy(() => unsub.forEach((u) => u()));

	const sorted = $derived(
		[...items].sort(
			(a, b) => ORDER.indexOf(a.status) - ORDER.indexOf(b.status) || b.updatedAt - a.updatedAt
		)
	);

	// 服务端的 clear 连 error 一起删，没有可清项时按钮禁用
	const hasClearable = $derived(
		items.some((it) => it.status === 'finished' || it.status === 'error')
	);

	async function toggleRunning() {
		busy = true;
		try {
			const res = await (running ? stop : start).mutateAsync();
			running = res.running;
		} finally {
			busy = false;
		}
	}

	async function clearFinished() {
		busy = true;
		try {
			// 服务端的 clear 连 error 一起删，本地跟齐
			const res = await clear.mutateAsync();
			applyItems(items.filter((it) => it.status !== 'finished' && it.status !== 'error'));
			pushToast($_('downloads.cleared', { values: { n: res.cleared } }), 'info');
		} finally {
			busy = false;
		}
	}
</script>

<div class="page container">
	<PageHeader title={$_('nav.downloads')}>
		{#snippet actions()}
			<span
				class={online ? 'ws-dot ws-dot--on' : 'ws-dot'}
				title={online ? $_('downloads.wsOnline') : $_('downloads.wsOffline')}
			></span>
			<Button
				variant={running ? 'ghost' : 'primary'}
				disabled={busy}
				onclick={() => void toggleRunning()}
			>
				{#if running}<Pause size={14} />{:else}<Play size={14} />{/if}
				{running ? $_('downloads.pause') : $_('downloads.start')}
			</Button>
			<Button disabled={busy || !hasClearable} onclick={() => void clearFinished()}>
				{$_('downloads.clearFinished')}
			</Button>
		{/snippet}
	</PageHeader>

	{#if loading}
		<div class="downloads-list">
			{#each Array.from({ length: 4 }) as _item, i (i)}
				<Skeleton height={72} />
			{/each}
		</div>
	{:else if sorted.length === 0}
		<EmptyState
			icon={Download}
			title={$_('downloads.emptyTitle')}
			hint={$_('downloads.emptyHint')}
		/>
	{:else}
		<div class="downloads-list">
			{#each sorted as it (it.chapterId)}
				{@const meta = STATUS_META[it.status]}
				{@const chapterName = chapterLabel({
					volume_number: it.volumeNumber,
					chapter_number: it.chapterNumber,
					title: it.chapterTitle
				})}
				{@const total = it.total ?? 0}
				{@const downloaded = it.downloaded ?? 0}
				<div class="downloads-item card">
					<div class="downloads-item__main">
						<div class="downloads-item__row">
							<span class="downloads-item__manga">{it.mangaTitle}</span>
							<Chip color={meta.color}>{$_(meta.key)}</Chip>
						</div>
						<div class="muted downloads-item__chapter">{chapterName}</div>
						{#if it.status === 'downloading'}
							<div class="downloads-item__progress">
								<ProgressBar value={total > 0 ? downloaded / total : null} />
								<span class="faint downloads-item__pv">
									{formatDownloadProgress({ downloaded, total })}
								</span>
							</div>
						{/if}
						{#if it.status === 'error'}
							<div class="downloads-item__error">
								{it.error || $_('downloads.failed')}
								{#if it.retries > 0}{$_('downloads.retried', { values: { n: it.retries } })}{/if}
							</div>
						{/if}
					</div>
					<div class="downloads-item__acts">
						{#if it.status === 'error'}
							<IconButton
								label={$_('downloads.retry')}
								disabled={enqueue.isPending}
								onclick={() =>
									enqueue.mutate([it.chapterId], {
										onSuccess: () =>
											applyItems(
												items.map((x) =>
													x.chapterId === it.chapterId
														? { ...x, status: 'queued', error: null }
														: x
												)
											)
									})}
							>
								<RotateCw size={16} />
							</IconButton>
						{/if}
						{#if it.status === 'finished'}
							<a
								class="downloads-item__cbz"
								href={chapterFileUrl(it.chapterId)}
								download
								title={$_('downloads.cbzTitle')}
							>
								<Download size={14} /> {$_('downloads.cbz')}
							</a>
						{/if}
						{#if it.status !== 'error'}
							<IconButton
								label={$_('downloads.deleteTask')}
								variant="danger"
								disabled={del.isPending}
								onclick={() =>
									del.mutate(it.chapterId, {
										onSuccess: () => {
											applyItems(items.filter((x) => x.chapterId !== it.chapterId));
											pushToast($_('downloads.deleted'), 'info');
										}
									})}
							>
								<Trash2 size={16} />
							</IconButton>
						{/if}
					</div>
				</div>
			{/each}
		</div>
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

	.downloads-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.downloads-item {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.downloads-item__main {
		flex: 1;
		min-width: 0;
	}

	.downloads-item__row {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.downloads-item__manga {
		font-weight: 600;
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
	}

	.downloads-item__chapter {
		font-size: 13.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.downloads-item__progress {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-top: 8px;
	}

	.downloads-item__progress :global(.progress) {
		flex: 1;
	}

	.downloads-item__pv {
		font-size: 12px;
		flex: none;
	}

	.downloads-item__error {
		margin-top: 6px;
		font-size: 12.5px;
		color: var(--danger);
	}

	.downloads-item__acts {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: none;
	}

	.downloads-item__cbz {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 5px 10px;
		border-radius: var(--r-sm);
		border: 1px solid color-mix(in srgb, var(--success) 45%, transparent);
		color: var(--success);
		font-size: 13px;
		white-space: nowrap;
	}

	.downloads-item__cbz:hover {
		background: color-mix(in srgb, var(--success) 12%, transparent);
	}
</style>
