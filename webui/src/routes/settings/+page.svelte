<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '#lib/api';
	import SettingItem from '#lib/components/SettingItem.svelte';
	import { pushToast } from '#lib/toast.svelte';
	import type { Source, SourceSettings } from '#lib/types';

	// —— 服务器 KV 设置 ——
	interface KvRow {
		key: string;
		text: string; // JSON 文本，保存时再解析
		isNew?: boolean;
	}

	let kvRows = $state<KvRow[]>([]);
	let kvBusy = $state(false);

	function parseVal(text: string): unknown {
		try {
			return JSON.parse(text);
		} catch {
			// 不是合法 JSON 时按字符串存
			return text;
		}
	}

	async function saveKv() {
		kvBusy = true;
		try {
			const body: Record<string, unknown> = {};
			for (const r of kvRows) {
				const k = r.key.trim();
				if (k) body[k] = parseVal(r.text);
			}
			const saved = await api<Record<string, unknown>>('/settings', { method: 'PUT', body });
			kvRows = Object.entries(saved).map(([key, v]) => ({ key, text: JSON.stringify(v) }));
			pushToast('服务器设置已保存', 'info');
		} finally {
			kvBusy = false;
		}
	}

	function removeKvRow(i: number) {
		kvRows.splice(i, 1);
	}

	// —— 各源设置 ——
	let sources = $state<Source[]>([]);
	let openSrc = $state<number | null>(null);
	let srcSettings = $state<Record<number, SourceSettings>>({});
	let srcEdits = $state<Record<number, Record<string, unknown>>>({});
	let srcBusy = $state(false);

	async function toggleSrc(id: number) {
		if (openSrc === id) {
			openSrc = null;
			return;
		}
		openSrc = id;
		if (!srcSettings[id]) {
			try {
				srcSettings[id] = await api<SourceSettings>(`/sources/${id}/settings`);
			} catch {
				openSrc = null;
			}
		}
	}

	function editFor(id: number): Record<string, unknown> {
		if (!srcEdits[id]) srcEdits[id] = {};
		return srcEdits[id];
	}

	function hasEdits(id: number): boolean {
		return Object.keys(srcEdits[id] ?? {}).length > 0;
	}

	async function saveSrc(id: number) {
		const edits = srcEdits[id];
		if (!edits || Object.keys(edits).length === 0) return;
		srcBusy = true;
		try {
			await api(`/sources/${id}/settings`, { method: 'PUT', body: edits });
			const cur = srcSettings[id];
			if (cur) srcSettings[id] = { ...cur, values: { ...cur.values, ...edits } };
			srcEdits[id] = {};
			pushToast('已保存', 'info');
		} finally {
			srcBusy = false;
		}
	}

	onMount(async () => {
		try {
			const [kv, srcs] = await Promise.all([
				api<Record<string, unknown>>('/settings'),
				api<Source[]>('/sources')
			]);
			kvRows = Object.entries(kv).map(([key, v]) => ({ key, text: JSON.stringify(v) }));
			sources = srcs;
		} catch {
			// toast 已由请求封装弹出
		}
	});
</script>

<div class="page">
	<h1 style="margin-bottom: 14px; font-size: 20px;">设置</h1>

	<section class="card">
		<div class="row-between" style="margin-bottom: 10px;">
			<h2 style="font-size: 16px;">服务器设置</h2>
			<div style="display: flex; gap: 8px;">
				<button onclick={() => kvRows.push({ key: '', text: '', isNew: true })}>新增</button>
				<button class="primary" disabled={kvBusy} onclick={saveKv}>保存</button>
			</div>
		</div>
		{#if kvRows.length === 0}
			<p class="faint" style="margin: 0;">暂无配置项</p>
		{:else}
			<div class="kv">
				{#each kvRows as row, i (i)}
					<div class="kv-row">
						<input
							placeholder="键"
							bind:value={row.key}
							readonly={!row.isNew}
							class:readonly={!row.isNew}
						/>
						<input placeholder="值（JSON，字符串可裸写）" bind:value={row.text} />
						<button class="danger" aria-label="删除" onclick={() => removeKvRow(i)}>✕</button>
					</div>
				{/each}
			</div>
		{/if}
	</section>

	<h2 style="margin: 20px 0 10px; font-size: 16px;">源设置</h2>
	{#if sources.length === 0}
		<p class="faint">还没有安装任何源</p>
	{:else}
		<div class="srcs">
			{#each sources as s (s.id)}
				<div class="card src">
					<button class="src-head" onclick={() => toggleSrc(s.id)}>
						<span>
							<strong>{s.name}</strong>
							<span class="faint" style="margin-left: 8px; font-size: 12.5px;">v{s.version}</span>
						</span>
						<span class="faint">{openSrc === s.id ? '▴' : '▾'}</span>
					</button>
					{#if openSrc === s.id}
						{@const data = srcSettings[s.id]}
						{#if !data}
							<p class="muted"><span class="spin"></span> 加载中…</p>
						{:else if !data.settings || data.settings.length === 0}
							<p class="faint" style="margin: 8px 0 0;">该源没有可配置项</p>
						{:else}
							<div class="src-body">
								{#each data.settings as setting (setting.key)}
									<SettingItem
										{setting}
										values={data.values}
										edits={editFor(s.id)}
										onedit={(k, v) => (editFor(s.id)[k] = v)}
									/>
								{/each}
								<div style="display: flex; justify-content: flex-end; margin-top: 8px;">
									<button class="primary" disabled={srcBusy || !hasEdits(s.id)} onclick={() => saveSrc(s.id)}>
										保存
									</button>
								</div>
							</div>
						{/if}
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	section.card {
		padding: 16px;
	}

	.kv {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.kv-row {
		display: grid;
		grid-template-columns: minmax(120px, 220px) 1fr auto;
		gap: 8px;
		align-items: center;
	}

	.kv-row input.readonly {
		color: var(--text-dim);
	}

	.srcs {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.src {
		padding: 4px 16px 14px;
	}

	.src-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 100%;
		border: none;
		background: none;
		padding: 12px 0;
		font-size: 15px;
	}

	.src-body {
		border-top: 1px solid var(--border);
		padding-top: 6px;
	}
</style>
