<script lang="ts">
	import { onMount } from 'svelte';
	import { api, chapterFileUrl } from '#lib/api';
	import { pushToast } from '#lib/toast.svelte';
	import { subscribeWS } from '#lib/ws';
	import type { DownloadItem, DownloadStatus } from '#lib/types';

	let items = $state<DownloadItem[]>([]);
	let online = $state(false);
	let loading = $state(true);
	let busy = $state(false);

	const ORDER: DownloadStatus[] = ['queued', 'downloading', 'paused', 'error', 'finished'];
	const STATUS_LABEL: Record<DownloadStatus, string> = {
		queued: '排队中',
		downloading: '下载中',
		paused: '已暂停',
		error: '失败',
		finished: '已完成'
	};
	const EVENT_STATUS: Record<string, DownloadStatus> = {
		queued: 'queued',
		started: 'downloading',
		// progress 帧也可能先于 started 到达，有进度即在下载
		progress: 'downloading',
		paused: 'paused',
		finished: 'finished',
		error: 'error'
	};

	let sorted = $derived(
		[...items].sort(
			(a, b) => ORDER.indexOf(a.status) - ORDER.indexOf(b.status) || b.updatedAt - a.updatedAt
		)
	);

	async function refresh() {
		items = await api<DownloadItem[]>('/downloads');
	}

	// 事件只带 chapterId，遇到没见过的任务要整表拉一次补齐标题；并发事件合并成一次
	let refetching = false;
	async function refreshMissing() {
		if (refetching) return;
		refetching = true;
		try {
			await refresh();
		} finally {
			refetching = false;
		}
	}

	function num(v: unknown): number | undefined {
		return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
	}

	function onMessage(msg: unknown) {
		// 快照帧兼容三种形态：{type:'snapshot',downloads|items:[...]} 或直接数组
		if (Array.isArray(msg)) {
			items = msg as DownloadItem[];
			loading = false;
			return;
		}
		if (!msg || typeof msg !== 'object') return;
		const m = msg as Record<string, unknown>;
		if (m.type === 'snapshot') {
			const list = Array.isArray(m.downloads) ? m.downloads : Array.isArray(m.items) ? m.items : [];
			items = list as DownloadItem[];
			loading = false;
			return;
		}
		const chapterId = num(m.chapterId);
		if (chapterId === undefined || typeof m.type !== 'string') return;
		if (!items.some((it) => it.chapterId === chapterId)) {
			void refreshMissing();
			return;
		}
		const patch: Partial<DownloadItem> = {};
		const status = EVENT_STATUS[m.type];
		if (status) patch.status = status;
		const downloaded = num(m.downloaded);
		const total = num(m.total);
		if (downloaded !== undefined) patch.downloaded = downloaded;
		if (total !== undefined) patch.total = total;
		if (m.type === 'finished' && patch.total !== undefined && patch.downloaded === undefined) {
			patch.downloaded = patch.total;
		}
		if (Object.keys(patch).length === 0) return;
		items = items.map((it) => (it.chapterId === chapterId ? { ...it, ...patch } : it));
	}

	async function start() {
		busy = true;
		try {
			await api('/downloads/start', { method: 'POST' });
		} finally {
			busy = false;
		}
	}

	async function stop() {
		busy = true;
		try {
			await api('/downloads/stop', { method: 'POST' });
		} finally {
			busy = false;
		}
	}

	async function clearFinished() {
		busy = true;
		try {
			const res = await api<{ cleared: number }>('/downloads/clear', { method: 'POST' });
			// 服务端的 clear 连 error 一起删，本地跟齐
			items = items.filter((it) => it.status !== 'finished' && it.status !== 'error');
			pushToast(`已清空 ${res.cleared} 项`, 'info');
		} finally {
			busy = false;
		}
	}

	async function remove(it: DownloadItem) {
		await api(`/downloads/${it.chapterId}`, { method: 'DELETE' });
		items = items.filter((x) => x.chapterId !== it.chapterId);
		pushToast('已删除', 'info');
	}

	onMount(() => {
		const unsub = subscribeWS('/api/v1/downloads/ws', {
			onMessage,
			onStatus: (v) => (online = v)
		});
		void (async () => {
			try {
				await refresh();
			} finally {
				loading = false;
			}
		})();
		return unsub;
	});
</script>

<svelte:head>
	<title>下载 - Suzuri</title>
</svelte:head>

<div class="page">
	<div class="bar">
		<h1 style="font-size: 20px;">下载</h1>
		<div class="ops">
			<button class="primary" disabled={busy} onclick={start}>开始</button>
			<button disabled={busy} onclick={stop}>暂停</button>
			<button disabled={busy} onclick={clearFinished}>清空已完成</button>
		</div>
		<span class="dot" class:on={online} title={online ? '实时连接正常' : '实时连接断开'}></span>
	</div>

	{#if loading}
		<div class="list">
			{#each Array(4) as _, i (i)}
				<div class="skeleton" style="height: 72px;"></div>
			{/each}
		</div>
	{:else if sorted.length === 0}
		<div class="empty">没有下载任务</div>
	{:else}
		<div class="list">
			{#each sorted as it (it.chapterId)}
				<div class="item card">
					<div class="main">
						<div class="row1">
							<span class="mt">{it.mangaTitle}</span>
							<span class="badge {it.status}">{STATUS_LABEL[it.status]}</span>
						</div>
						<div class="muted ct">{it.chapterTitle}</div>
						{#if it.status === 'downloading'}
							<div class="progress">
								<div class="track">
									{#if (it.total ?? 0) > 0}
										<div
											class="fill"
											style="width: {Math.min(100, ((it.downloaded ?? 0) / (it.total ?? 1)) * 100)}%;"
										></div>
									{:else}
										<div class="fill indet"></div>
									{/if}
								</div>
								<span class="faint pv">
									{#if (it.total ?? 0) > 0}{it.downloaded ?? 0} / {it.total}{:else}{it.downloaded ?? 0}{/if}
								</span>
							</div>
						{/if}
						{#if it.status === 'error'}
							<div class="errtext">
								{it.error || '下载失败'}{#if it.retries > 0}（已重试 {it.retries} 次）{/if}
							</div>
						{/if}
					</div>
					<div class="acts">
						{#if it.status === 'finished'}
							<a class="cbz" href={chapterFileUrl(it.chapterId)} download title="下载 CBZ 文件">
								⬇ CBZ
							</a>
						{/if}
						<button class="del" title="删除任务" onclick={() => remove(it)}>✕</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.bar {
		display: flex;
		align-items: center;
		gap: 14px;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}

	.ops {
		display: flex;
		gap: 8px;
		flex: 1;
		flex-wrap: wrap;
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

	.list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.item {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.main {
		flex: 1;
		min-width: 0;
	}

	.row1 {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.mt {
		font-weight: 600;
		font-size: 14.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		min-width: 0;
	}

	.badge {
		flex: none;
		padding: 2px 9px;
		border-radius: 999px;
		font-size: 12px;
		border: 1px solid var(--border);
		color: var(--text-dim);
		background: var(--bg-raised);
	}

	.badge.downloading {
		color: var(--accent);
		border-color: var(--accent-dim);
		background: color-mix(in srgb, var(--accent) 16%, var(--bg-raised));
	}

	.badge.error {
		color: var(--danger);
		border-color: color-mix(in srgb, var(--danger) 55%, transparent);
		background: color-mix(in srgb, var(--danger) 15%, var(--bg-raised));
	}

	.badge.finished {
		color: var(--ok);
		border-color: color-mix(in srgb, var(--ok) 45%, transparent);
		background: color-mix(in srgb, var(--ok) 13%, var(--bg-raised));
	}

	.ct {
		font-size: 13.5px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.progress {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-top: 8px;
	}

	.track {
		flex: 1;
		height: 6px;
		border-radius: 999px;
		background: var(--bg);
		overflow: hidden;
	}

	.fill {
		height: 100%;
		background: var(--accent);
		border-radius: 999px;
		transition: width 0.3s;
	}

	.fill.indet {
		width: 40%;
		animation: slide 1.2s ease-in-out infinite;
	}

	@keyframes slide {
		0% {
			margin-left: 0;
		}
		50% {
			margin-left: 60%;
		}
		100% {
			margin-left: 0;
		}
	}

	.pv {
		font-size: 12px;
		flex: none;
	}

	.errtext {
		margin-top: 6px;
		font-size: 12.5px;
		color: var(--danger);
	}

	.acts {
		display: flex;
		align-items: center;
		gap: 6px;
		flex: none;
	}

	.cbz {
		padding: 5px 10px;
		border-radius: 8px;
		border: 1px solid color-mix(in srgb, var(--ok) 45%, transparent);
		color: var(--ok);
		font-size: 13px;
		white-space: nowrap;
	}

	.cbz:hover {
		background: color-mix(in srgb, var(--ok) 12%, transparent);
	}

	.del {
		border: none;
		background: none;
		color: var(--text-faint);
		font-size: 14px;
		padding: 6px 8px;
	}

	.del:hover {
		color: var(--danger);
		background: none;
	}
</style>
