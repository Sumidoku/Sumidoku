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
	import type { DownloadItem, DownloadStatus, DownloadWsFrame } from '$lib/api/types';
	import { subscribeWS } from '$lib/api/ws';
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

	// progress 帧也可能先于 started 到达，有进度即在下载
	const EVENT_STATUS: Record<string, DownloadStatus> = {
		queued: 'queued',
		started: 'downloading',
		progress: 'downloading',
		paused: 'paused',
		finished: 'finished',
		error: 'error'
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

	// $state 代理在 WS 回调里读取即是最新值，无需 React 版的 itemsRef
	function applyItems(next: DownloadItem[]) {
		items = next;
	}

	// 事件只带 chapterId，遇到没见过的任务要整表拉一次补齐标题；并发事件合并成一次
	let refetching = false;
	async function refreshMissing() {
		if (refetching) return;
		refetching = true;
		try {
			applyItems(await api<DownloadItem[]>('/downloads'));
		} finally {
			refetching = false;
		}
	}

	function onMessage(msg: unknown) {
		if (!msg || typeof msg !== 'object' || Array.isArray(msg)) return;
		const m = msg as DownloadWsFrame;
		if (m.type === 'snapshot') {
			applyItems(m.downloads);
			running = m.running;
			loading = false;
			return;
		}
		if (m.type === 'running') {
			running = m.running;
			return;
		}
		const status = EVENT_STATUS[m.type];
		if (!status) return;
		if (!items.some((it) => it.chapterId === m.chapterId)) {
			void refreshMissing();
			return;
		}
		applyItems(
			items.map((it) => {
				if (it.chapterId !== m.chapterId) return it;
				const patch: Partial<DownloadItem> = { status };
				if (status === 'downloading' || status === 'finished') {
					patch.downloaded = m.downloaded;
					patch.total = m.total;
					if (status === 'finished') {
						patch.error = null;
						if (m.total > 0) patch.downloaded = m.total;
					}
				} else {
					patch.downloaded = null;
					patch.total = null;
					if (status === 'queued') patch.error = null;
				}
				return { ...it, ...patch };
			})
		);
	}

	let unsub: (() => void) | undefined;
	onMount(() => {
		unsub = subscribeWS('/api/v1/downloads/ws', {
			onMessage,
			onStatus: (o) => (online = o)
		});
		void (async () => {
			try {
				applyItems(await api<DownloadItem[]>('/downloads'));
			} finally {
				loading = false;
			}
		})();
	});
	onDestroy(() => unsub?.());

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
				{@const chapterName = it.chapterTitle?.trim() || $_('downloads.untitledChapter')}
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
									{total > 0 ? `${downloaded} / ${total}` : `${downloaded}`}
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
